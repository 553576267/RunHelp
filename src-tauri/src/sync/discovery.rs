//! 局域网设备发现：UDP 广播 + 收到后的单播回应。
//! 每个实例每秒广播一次自身信息，同时监听同端口接收其它实例广播。
//! 广播包格式：`RUNHELP1|device_id|name(hex)|platform|port|HELLO`
//! - HELLO：向 255.255.255.255 广播；多网卡时 Windows 只从默认网卡发出，可能单向。
//! - HERE：收到 HELLO 后向源 IP 单播回应。单播路由明确，能穿透多网卡导致的单向问题。
//! 末尾类型字段为可选（旧版 5 段包按 HELLO 处理），保证混版兼容。
//! 名称用 hex 编码，避免竖线/换行破坏协议，也天然支持中文。

use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream, UdpSocket};
use std::sync::atomic::Ordering;
use std::sync::{mpsc, Arc};
use std::time::Duration;

use super::{DeviceInfo, SyncRuntime, DISCOVERY_MAGIC, DISCOVERY_PORT, TRANSFER_PORT};

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
    let mut hello_packet = build_packet(&device_id, &name, &platform, port, KIND_HELLO, &fingerprint);
    let mut here_packet = build_packet(&device_id, &name, &platform, port, KIND_HERE, &fingerprint);
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
            // 包体跟着「当前设备名」走：改名后在下一轮广播就生效，
            // 不必重启同步服务（以前包体是启动时构造的固定字节，对方永远看到旧名字）。
            let mut cur_name = name;

            while !stop_thread.load(Ordering::SeqCst) {
                // 每秒广播一次
                if last_broadcast.elapsed() >= Duration::from_secs(1) {
                    let latest = rt.self_name.lock().map(|s| s.clone()).unwrap_or_default();
                    if !latest.is_empty() && latest != cur_name {
                        cur_name = latest;
                        hello_packet = build_packet(&local_id, &cur_name, &platform, port, KIND_HELLO, &fingerprint);
                        here_packet = build_packet(&local_id, &cur_name, &platform, port, KIND_HERE, &fingerprint);
                    }
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
                                    let removed = rt
                                        .devices
                                        .lock()
                                        .ok()
                                        .and_then(|mut devices| devices.remove(bye_id));
                                    if let Some(d) = removed {
                                        // BYE 只说明「对方主动关了同步服务」，是真离线，不用等超时。
                                        super::commands::note_state(
                                            &rt, "warn", &d.name,
                                            format!("[设备] {} 离线 · 收到关闭通知(BYE) · ip {}", d.name, d.ip),
                                        );
                                        // 移除也算列表变化，立刻让前端刷新，别等 5 秒轮询。
                                        rt.device_dirty.store(true, Ordering::SeqCst);
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
                                let outcome = upsert_device(&rt, id, dev_name, dev_platform, ip, dev_port, dev_fp);
                                if outcome.is_new || outcome.went_online {
                                    super::commands::note_state(
                                        &rt, "info", &outcome.name,
                                        format!(
                                            "[设备] {} {} · 收到广播 · ip {}",
                                            outcome.name,
                                            if outcome.is_new { "上线（新发现）" } else { "重新上线" },
                                            outcome.ip
                                        ),
                                    );
                                }
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

                // 清理超时设备：常规设备 4 秒（广播）+ 8 秒（探测）；只靠扫描发现的给足 45 秒。
                // 清理时把「为什么离线」记进诊断日志，这是区分「信号丢了」和「逻辑判错」的关键证据。
                let now = super::now_secs();
                let mut dropped: Vec<(String, String, String, String, bool, bool)> = Vec::new();
                if let Ok(mut devices) = rt.devices.lock() {
                    devices.retain(|_, d| {
                        if d.is_online(now) {
                            return true;
                        }
                        dropped.push((
                            d.name.clone(),
                            d.ip.clone(),
                            super::age_desc(now, d.last_seen),
                            super::age_desc(now, d.last_probe),
                            d.probe_only,
                            // 已经由探测线程/状态扫描判过离线并记过日志的，这里只清不重复记
                            d.online,
                        ));
                        false
                    });
                    // 有设备因超时被清掉：也要推一次事件，否则界面上会「幽灵」停留到下次轮询。
                    if !dropped.is_empty() {
                        rt.device_dirty.store(true, Ordering::SeqCst);
                    }
                }
                for (name, ip, hb, probe, probe_only, was_online) in dropped {
                    if !was_online {
                        continue;
                    }
                    super::commands::note_state(
                        &rt, "warn", &name,
                        format!(
                            "[设备] {} 离线 · 心跳/探测时间戳均超时（{}）· 心跳 {} / 探测 {} · ip {}",
                            name,
                            if probe_only { "仅扫描发现" } else { "广播+探测" },
                            hb,
                            probe,
                            ip
                        ),
                    );
                }
            }
            let _ = socket.send_to(bye_packet.as_bytes(), broadcast_addr);
        })
        .map_err(|e| format!("启动发现线程失败: {}", e))?;

    Ok(stop)
}

/// 占位名称：探测拿不到真名时的兜底值，不能覆盖已有真实名称。
fn is_placeholder_name(name: &str) -> bool {
    let n = name.trim();
    n.is_empty() || n == "未知设备"
}

/// 占位平台：探测拿不到平台时的兜底值，不能覆盖已有真实平台。
fn is_placeholder_platform(platform: &str) -> bool {
    let p = platform.trim();
    p.is_empty() || p == "unknown"
}

/// upsert 的结果：调用方据此决定要不要记一条上下线诊断日志。
/// 发现线程/扫描线程/探测线程各自有不同「上线原因」，日志必须由它们自己写，
/// 所以这里只回传事实，不在这里记日志。
pub struct UpsertOutcome {
    /// 这是第一次见到该设备
    pub is_new: bool,
    /// 之前是离线（或不存在），这次转成在线
    pub went_online: bool,
    pub name: String,
    pub fingerprint: String,
    pub ip: String,
}

/// 广播发现的常规设备（4 秒心跳超时）
pub fn upsert_device(
    rt: &SyncRuntime,
    id: String,
    name: String,
    platform: String,
    ip: String,
    port: u16,
    fingerprint: String,
) -> UpsertOutcome {
    upsert_device_ex(rt, id, name, platform, ip, port, fingerprint, false)
}

/// 网段探测发现的设备：无广播心跳，用更长的离线超时。
pub fn upsert_device_probe(
    rt: &SyncRuntime,
    id: String,
    name: String,
    platform: String,
    ip: String,
    port: u16,
    fingerprint: String,
) -> UpsertOutcome {
    upsert_device_ex(rt, id, name, platform, ip, port, fingerprint, true)
}

fn upsert_device_ex(
    rt: &SyncRuntime,
    id: String,
    name: String,
    platform: String,
    ip: String,
    port: u16,
    fingerprint: String,
    probe_only: bool,
) -> UpsertOutcome {
    let now = super::now_secs();
    // 广播发现刷新 last_seen，探测发现只刷新 last_probe：
    // 两个时间戳分开，才能既容忍广播丢包，又让探测结果维持在线（详见 DeviceInfo::is_online）。
    if let Ok(mut devices) = rt.devices.lock() {
        // 同一台设备换了 id（重装应用、恢复备份后 id 重建）：指纹才是设备身份证，
        // 顺手清掉旧 id 遗留的**离线**条目，避免列表里留着一条永远不再更新的幽灵。
        // 只清离线条目：在线条目若与当前 id 同指纹，说明是本机识别出了问题（不该发生），
        // 贸然互删会让两个来源每包来回插删，反而变成新的抖动源。
        if !fingerprint.is_empty() {
            let stale: Vec<String> = devices
                .iter()
                .filter(|(k, d)| {
                    k.as_str() != id && !d.fingerprint.is_empty() && d.fingerprint == fingerprint && !d.online
                })
                .map(|(k, _)| k.clone())
                .collect();
            for k in stale {
                devices.remove(&k);
            }
        }
        if let Some(existing) = devices.get_mut(&id) {
            // 由离线转为在线也算「列表有变化」：前端要在 1~2 秒内看到它亮起来，
            // 不能等 5 秒一次的兜底轮询。
            let went_online = !existing.online;
            if went_online {
                rt.device_dirty.store(true, Ordering::SeqCst);
            }
            // 名字来源有两个：UDP 广播（旧版对端是启动时的固定包，改名后仍是旧名）
            // 与 `/hello` 探测（实时读库，永远最新）。探测名在保鲜期内优先，
            // 否则每秒一包的广播会一直把新名字改回旧名字。
            let probe_name_fresh =
                existing.name_probe_at > 0 && now - existing.name_probe_at < super::NAME_PROBE_HOLD_SECS;
            if !is_placeholder_name(&name) && (probe_only || !probe_name_fresh) {
                existing.name = name.clone();
            }
            if probe_only && !is_placeholder_name(&name) {
                existing.name_probe_at = now;
            }
            if !is_placeholder_platform(&platform) {
                existing.platform = platform;
            }
            existing.ip = ip;
            existing.port = port;
            if probe_only {
                existing.last_probe = now;
                // 仅探测发现的设备没有心跳，last_seen 也要跟着走，否则 45 秒兜底超时算不准。
                if existing.probe_only {
                    existing.last_seen = now;
                }
            } else {
                existing.last_seen = now;
            }
            existing.online = true;
            // 只要收到过广播心跳就转回常规；仅当两次都是探测发现才保持 probe_only。
            existing.probe_only = existing.probe_only && probe_only;
            if !fingerprint.is_empty() {
                existing.fingerprint = fingerprint;
            }
            return UpsertOutcome {
                is_new: false,
                went_online,
                name: existing.name.clone(),
                fingerprint: existing.fingerprint.clone(),
                ip: existing.ip.clone(),
            };
        }
    }
    // 可信状态不在这里读库：发现层只维护「谁在线」，
    // 可信/收藏/别名由命令层在列出设备时按指纹合并 sync_devices 表。
    let outcome = UpsertOutcome {
        is_new: true,
        went_online: true,
        name: name.clone(),
        fingerprint: fingerprint.clone(),
        ip: ip.clone(),
    };
    let device = DeviceInfo {
        id: id.clone(),
        name,
        platform,
        ip: ip.clone(),
        port,
        fingerprint,
        last_seen: now,
        last_probe: if probe_only { now } else { 0 },
        trusted: false,
        auto_accept: false,
        favorite: false,
        alias: String::new(),
        last_ip: ip,
        online: true,
        probe_only,
        name_probe_at: if probe_only { now } else { 0 },
    };
    if let Ok(mut devices) = rt.devices.lock() {
        devices.insert(id, device);
    }
    // 新设备第一次出现：立刻通知前端刷新，不必等兜底轮询。
    rt.device_dirty.store(true, Ordering::SeqCst);
    outcome
}

// ===== 网段主动探测（热点/AP 隔离下的发现兜底） =====
//
// UDP 广播 255.255.255.255 在手机热点、部分路由器上被隔离，导致设备互相发现不到。
// 这里额外对每个本机网卡所在的 /24 网段做主动探测：先 TCP 快筛开放传输端口的 IP，
// 再用 mTLS /hello 拿身份。热点场景手机就是网关 .1，必然被覆盖。
// 与 LocalSend 的 legacy 模式（遍历子网发 register）思路一致。

/// 拿本机所有活跃网卡的 IPv4（不真正发包，仅靠 UDP connect 让内核选出接口 IP）。
/// 多个目标用于触发多网卡各自暴露；去重后返回。
fn local_ipv4s() -> Vec<Ipv4Addr> {
    let targets = [
        "8.8.8.8:47800",
        "114.114.114.114:47800",
        "223.5.5.5:47800",
        "192.168.1.1:47800",
        "10.0.0.1:47800",
    ];
    let mut ips: Vec<Ipv4Addr> = Vec::new();
    for target in targets {
        if let Ok(sock) = UdpSocket::bind("0.0.0.0:0") {
            if sock.connect(target).is_ok() {
                if let Ok(addr) = sock.local_addr() {
                    if let IpAddr::V4(v4) = addr.ip() {
                        if !v4.is_unspecified() && !v4.is_loopback() && !ips.contains(&v4) {
                            ips.push(v4);
                        }
                    }
                }
            }
        }
    }
    ips
}

/// 每秒广播启动的同时拉起网段扫描线程。返回其停止标志（与广播共用 rt.discovery_stop）。
pub fn start_subnet_scan(rt: Arc<SyncRuntime>, self_id: String) {
    let stop = rt.discovery_stop.clone();
    let identity = rt.identity.clone();
    let scan_stop = stop.clone();

    std::thread::Builder::new()
        .name("runhelp-sync-subnet-scan".to_string())
        .spawn(move || {
            // 首轮立即扫，之后每 SUBNET_SCAN_INTERVAL_SECS 秒一轮；扫描本身耗时通常 < 2 秒。
            // 间隔别调太密：跳数越多越容易和 600ms/900ms 的存活探测抢射频，反而制造假离线。
            loop {
                if scan_stop.load(Ordering::SeqCst) {
                    break;
                }
                scan_round(&rt, &identity, &self_id, &scan_stop);
                // 分段 sleep，便于停止标志及时生效
                for _ in 0..super::SUBNET_SCAN_INTERVAL_SECS {
                    if scan_stop.load(Ordering::SeqCst) {
                        return;
                    }
                    std::thread::sleep(Duration::from_secs(1));
                }
            }
        })
        .map_err(|e| {
            // 扫描线程启动失败不应阻断发现服务，仅记录到 stderr。
            eprintln!("启动网段扫描线程失败: {}", e);
        })
        .ok();
}

fn scan_round(
    rt: &Arc<SyncRuntime>,
    identity: &super::identity::Identity,
    self_id: &str,
    stop: &Arc<std::sync::atomic::AtomicBool>,
) {
    // 身份头里的名字每轮现取：改名后不用重启也能让对方看到新名字。
    let self_name = rt.self_name.lock().map(|s| s.clone()).unwrap_or_default();
    let self_name = if self_name.trim().is_empty() {
        super::commands::display_name_ro()
    } else {
        self_name
    };
    let self_name = self_name.as_str();
    let locals = local_ipv4s();
    if locals.is_empty() {
        return;
    }
    // 收集本机所有 /24 网段内的候选 IP（跳过 .0/.255 与本机自身）。
    let mut targets: Vec<Ipv4Addr> = Vec::new();
    for local in &locals {
        let oct = local.octets();
        for last in 1u8..=254 {
            let cand = Ipv4Addr::new(oct[0], oct[1], oct[2], last);
            if locals.contains(&cand) {
                continue;
            }
            if !targets.contains(&cand) {
                targets.push(cand);
            }
        }
    }
    if targets.is_empty() {
        return;
    }

    // 有界并发：把候选 IP 投入 channel，固定 N 个 worker 领取。
    const WORKERS: usize = 32;
    let (tx, rx) = mpsc::channel::<Ipv4Addr>();
    {
        let tx = tx;
        for ip in targets {
            if tx.send(ip).is_err() {
                break;
            }
        }
    } // drop tx，worker 收完即退出

    let rx = Arc::new(std::sync::Mutex::new(rx));
    let mut handles = Vec::new();
    for _ in 0..WORKERS {
        let rx = rx.clone();
        let rt = rt.clone();
        let identity = identity.clone();
        let self_id = self_id.to_string();
        let self_name = self_name.to_string();
        let stop = stop.clone();
        if let Ok(h) = std::thread::Builder::new().spawn(move || {
            loop {
                if stop.load(Ordering::SeqCst) {
                    break;
                }
                let ip = {
                    let guard = match rx.lock() {
                        Ok(g) => g,
                        Err(_) => break,
                    };
                    match guard.recv_timeout(Duration::from_millis(50)) {
                        Ok(v) => v,
                        Err(mpsc::RecvTimeoutError::Timeout) => continue,
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                };
                // TCP 快筛：只探测开放传输端口的 IP，避免对整段做 mTLS 握手。
                let addr = SocketAddr::from((ip, TRANSFER_PORT));
                if TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_err() {
                    continue;
                }
                let ip_str = ip.to_string();
                if let Ok(info) =
                    super::transfer::probe_scan(&ip_str, TRANSFER_PORT, &identity, &self_id, &self_name)
                {
                    if !info.id.is_empty() && !info.fingerprint.is_empty() {
                        let outcome = upsert_device_probe(
                            &rt,
                            info.id,
                            if info.name.is_empty() { "未知设备".to_string() } else { info.name },
                            if info.platform.is_empty() { "unknown".to_string() } else { info.platform },
                            ip_str.clone(),
                            TRANSFER_PORT,
                            info.fingerprint,
                        );
                        // 只在「首次发现/由离线转在线」时记一行，避免每 20 秒刷屏。
                        if outcome.is_new || outcome.went_online {
                            super::commands::note_state(
                                &rt, "info", &outcome.name,
                                format!(
                                    "[设备] {} {} · 网段扫描发现（广播可能被 AP 隔离）· ip {}",
                                    outcome.name,
                                    if outcome.is_new { "上线（新发现）" } else { "重新上线" },
                                    outcome.ip
                                ),
                            );
                        }
                    }
                }
            }
        }) {
            handles.push(h);
        }
    }
    for h in handles {
        let _ = h.join();
    }
}
