//! 局域网设备发现：UDP 广播 + 收到后的单播回应。
//! 每个实例每秒广播一次自身信息，同时监听同端口接收其它实例广播。
//! 广播包格式：`RUNHELP1|device_id|name(hex)|platform|port|HELLO`
//! - HELLO：向 255.255.255.255 广播；多网卡时 Windows 只从默认网卡发出，可能单向。
//! - HERE：收到 HELLO 后向源 IP 单播回应。单播路由明确，能穿透多网卡导致的单向问题。
//! 末尾类型字段为可选（旧版 5 段包按 HELLO 处理），保证混版兼容。
//! 名称用 hex 编码，避免竖线/换行破坏协议，也天然支持中文。

use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use super::{DeviceInfo, SyncRuntime, DISCOVERY_MAGIC, DISCOVERY_PORT, DEVICE_TIMEOUT_SECS};

fn hex_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 2);
    for b in s.as_bytes() {
        out.push_str(&format!("{:02x}", b));
    }
    out
}

fn hex_decode(s: &str) -> Option<String> {
    if s.len() % 2 != 0 {
        return None;
    }
    let bytes: Result<Vec<u8>, _> = (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16))
        .collect();
    bytes.ok().and_then(|b| String::from_utf8(b).ok())
}

/// 包类型：广播宣告 / 单播回应 / 关闭通知
const KIND_HELLO: &str = "HELLO";
const KIND_HERE: &str = "HERE";
const KIND_BYE: &str = "BYE";

/// 组装广播包（kind 为 HELLO 或 HERE，末尾附证书指纹）
fn build_packet(id: &str, name: &str, platform: &str, port: u16, kind: &str, fp: &str) -> String {
    format!(
        "{}|{}|{}|{}|{}|{}|{}",
        DISCOVERY_MAGIC,
        id,
        hex_encode(name),
        hex_encode(platform),
        port,
        kind,
        fp
    )
}

/// 解析广播包，返回 (id, name, platform, port, kind, fingerprint)。
/// 旧版 5/6 段包没有指纹，返回空串以兼容混版。
fn parse_packet(packet: &str) -> Option<(String, String, String, u16, String, String)> {
    let parts: Vec<&str> = packet.trim().split('|').collect();
    if parts.len() < 5 || parts[0] != DISCOVERY_MAGIC {
        return None;
    }
    let name = hex_decode(parts[2])?;
    let platform = hex_decode(parts[3])?;
    let port: u16 = parts[4].parse().ok()?;
    if name.is_empty() || parts[1].is_empty() {
        return None;
    }
    let kind = parts.get(5).filter(|k| !k.is_empty()).unwrap_or(&KIND_HELLO).to_string();
    let fp = parts.get(6).filter(|f| !f.is_empty()).unwrap_or(&"").to_string();
    Some((parts[1].to_string(), name, platform, port, kind, fp))
}

/// 启动发现服务：后台线程每秒广播 + 非阻塞接收。返回停止标志的克隆。
pub fn start(
    rt: Arc<SyncRuntime>,
    device_id: String,
    name: String,
    platform: String,
    port: u16,
    fingerprint: String,
) -> Result<Arc<std::sync::atomic::AtomicBool>, String> {
    let socket = UdpSocket::bind(("0.0.0.0", DISCOVERY_PORT))
        .map_err(|e| format!("绑定发现端口 {} 失败: {}", DISCOVERY_PORT, e))?;
    socket
        .set_broadcast(true)
        .map_err(|e| format!("开启 UDP 广播失败: {}", e))?;
    socket
        .set_read_timeout(Some(Duration::from_millis(300)))
        .map_err(|e| format!("设置接收超时失败: {}", e))?;

    let stop = rt.discovery_stop.clone();
    let stop_thread = stop.clone();
    // HELLO 走广播；HERE 走单播，收到对方 HELLO 后定向回一包，解决多网卡单向发现
    let hello_packet = build_packet(&device_id, &name, &platform, port, KIND_HELLO, &fingerprint);
    let here_packet = build_packet(&device_id, &name, &platform, port, KIND_HERE, &fingerprint);
    // 关闭通知带上自身 id，对方收到后立即从列表移除，无需等超时。
    let bye_packet = format!("{}|{}", KIND_BYE, device_id);
    let local_id = device_id.clone();

    std::thread::Builder::new()
        .name("runhelp-sync-discovery".to_string())
        .spawn(move || {
            let broadcast_addr = SocketAddr::from(([255, 255, 255, 255], DISCOVERY_PORT));
            let mut last_broadcast = std::time::Instant::now()
                .checked_sub(Duration::from_secs(1))
                .unwrap_or_else(std::time::Instant::now);

            while !stop_thread.load(Ordering::SeqCst) {
                // 每秒广播一次
                if last_broadcast.elapsed() >= Duration::from_secs(1) {
                    let _ = socket.send_to(hello_packet.as_bytes(), broadcast_addr);
                    last_broadcast = std::time::Instant::now();
                }

                // 接收其它设备广播 / 单播回应
                let mut buf = [0u8; 4096];
                match socket.recv_from(&mut buf) {
                    Ok((len, src)) => {
                        if let Ok(text) = std::str::from_utf8(&buf[..len]) {
                            let text = text.trim();
                            // 对方关闭软件时发出的 BYE：立即移除，刷新即刻反映离线。
                            if let Some(bye_id) = text.strip_prefix(&format!("{}|", KIND_BYE)) {
                                if bye_id != local_id {
                                    if let Ok(mut devices) = rt.devices.lock() {
                                        devices.remove(bye_id);
                                    }
                                }
                                continue;
                            }
                            if let Some((id, dev_name, dev_platform, dev_port, kind, dev_fp)) = parse_packet(text) {
                                // 忽略自己的包
                                if id == local_id {
                                    continue;
                                }
                                let ip = match src.ip() {
                                    IpAddr::V4(v4) => v4.to_string(),
                                    IpAddr::V6(v6) => v6.to_string(),
                                };
                                upsert_device(&rt, id, dev_name, dev_platform, ip.clone(), dev_port, dev_fp);
                                // 只对 HELLO 回 HERE；HERE 是回应，不再回，避免来回打乒乓
                                if kind == KIND_HELLO {
                                    // 用源 IP:端口回单播；src 已是 SocketAddr，直接把端口换成发现端口
                                    let mut reply_addr = src;
                                    reply_addr.set_port(DISCOVERY_PORT);
                                    let _ = socket.send_to(here_packet.as_bytes(), reply_addr);
                                }
                            }
                        }
                    }
                    Err(ref e)
                        if e.kind() == std::io::ErrorKind::WouldBlock
                            || e.kind() == std::io::ErrorKind::TimedOut => {}
                    Err(_) => {}
                }

                // 清理超时设备
                let now = super::now_secs();
                if let Ok(mut devices) = rt.devices.lock() {
                    devices.retain(|_, d| now - d.last_seen < DEVICE_TIMEOUT_SECS);
                }
            }
            let _ = socket.send_to(bye_packet.as_bytes(), broadcast_addr);
        })
        .map_err(|e| format!("启动发现线程失败: {}", e))?;

    Ok(stop)
}

/// 写入/更新设备（保留可信与自动接收标记）
pub fn upsert_device(
    rt: &SyncRuntime,
    id: String,
    name: String,
    platform: String,
    ip: String,
    port: u16,
    fingerprint: String,
) {
    if let Ok(mut devices) = rt.devices.lock() {
        if let Some(existing) = devices.get_mut(&id) {
            existing.name = name;
            existing.platform = platform;
            existing.ip = ip;
            existing.port = port;
            existing.last_seen = super::now_secs();
            existing.online = true;
            if !fingerprint.is_empty() {
                existing.fingerprint = fingerprint;
            }
            return;
        }
    }
    // 可信状态不在这里读库：发现层只维护「谁在线」，
    // 可信/收藏/别名由命令层在列出设备时按指纹合并 sync_devices 表。
    let device = DeviceInfo {
        id: id.clone(),
        name,
        platform,
        ip: ip.clone(),
        port,
        fingerprint,
        last_seen: super::now_secs(),
        trusted: false,
        auto_accept: false,
        favorite: false,
        alias: String::new(),
        last_ip: ip,
        online: true,
    };
    if let Ok(mut devices) = rt.devices.lock() {
        devices.insert(id, device);
    }
}
