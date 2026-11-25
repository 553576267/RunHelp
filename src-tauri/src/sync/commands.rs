//! 数据同步的 Tauri 命令与事件发射。

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tauri::{Emitter, Manager};

use super::discovery;
use super::identity;
use super::transfer;
use super::{DeviceInfo, ProgressEvent, ReceivedEvent, SyncRuntime};
use crate::state::AppState;

fn rt(app: &tauri::AppHandle) -> Arc<SyncRuntime> {
    app.state::<AppState>().sync.clone()
}

/// 本机显示名（供 HTTP /hello 使用，避免每次重新打开数据目录）
pub fn display_name_ro() -> String {
    AppState::open_db_for_sync()
        .as_ref()
        .map(|c| super::display_name(c))
        .unwrap_or_else(|| "RunHelp 设备".to_string())
}

/// 从设置读同步接收目录
pub fn receive_dir() -> PathBuf {
    // 只接受本地绝对路径；Android 的 content:// URI 或相对路径一律忽略，
    // 否则 base.join(rel) 会落到不可写位置，接收文件时返回 500。
    let configured = AppState::open_db_for_sync()
        .as_ref()
        .and_then(|c| {
            c.query_row(
                "SELECT value FROM settings WHERE key='sync_receive_dir'",
                [],
                |r| r.get::<_, String>(0),
            )
            .ok()
        })
        .filter(|s| !s.trim().is_empty())
        .map(PathBuf::from)
        .filter(|p| p.is_absolute() && !p.to_string_lossy().starts_with("content://"));

    let dir = configured.unwrap_or_else(|| {
        #[cfg(target_os = "android")]
        {
            // 移动端没有可靠的 HOME/USERPROFILE，使用应用私有可写目录。
            AppState::app_handle_for_sync()
                .and_then(|app| app.path().app_data_dir().ok())
                .map(|p| p.join("sync_receive"))
                .unwrap_or_else(std::env::temp_dir)
        }
        #[cfg(not(target_os = "android"))]
        {
            crate::platform::home_dir().join("Downloads").join("RunHelp同步")
        }
    });
    // 提前建目录；失败仍返回路径，由 receive_file 把具体 OS 错误透传给发送方。
    let _ = std::fs::create_dir_all(&dir);
    dir
}

pub fn is_trusted_fingerprint(fp: &str) -> bool {
    if fp.is_empty() {
        return false;
    }
    AppState::open_db_for_sync()
        .as_ref()
        .and_then(|c| {
            c.query_row(
                "SELECT trusted FROM sync_devices WHERE fingerprint=?1",
                [fp],
                |r| r.get::<_, i32>(0),
            )
            .ok()
        })
        .unwrap_or(0)
        != 0
}

pub fn auto_accept_fingerprint(fp: &str) -> bool {
    if fp.is_empty() {
        return false;
    }
    AppState::open_db_for_sync()
        .as_ref()
        .and_then(|c| {
            c.query_row(
                "SELECT auto_accept FROM sync_devices WHERE fingerprint=?1",
                [fp],
                |r| r.get::<_, i32>(0),
            )
            .ok()
        })
        .unwrap_or(0)
        != 0
}

/// TOFU：首次传输确认后记住对方指纹与身份。
pub fn trust_fingerprint(fp: &str, device_id: &str, name: &str) {
    trust_fingerprint_ex(fp, device_id, name, "");
}

/// 带平台的版本：把对端平台一并落库，离线后列表仍能显示设备类型而不是「未知设备」。
pub fn trust_fingerprint_ex(fp: &str, device_id: &str, name: &str, platform: &str) {
    if fp.is_empty() {
        return;
    }
    let platform = platform.trim();
    let has_platform = !platform.is_empty() && platform != "unknown";
    if let Some(conn) = AppState::open_db_for_sync() {
        // 先尝试按指纹更新已有记录
        let updated = if has_platform {
            conn.execute(
                "UPDATE sync_devices SET trusted=1, name=?2, device_id=?3, platform=?4, \
                 last_seen=datetime('now','localtime') WHERE fingerprint=?1",
                rusqlite::params![fp, name, device_id, platform],
            )
        } else {
            conn.execute(
                "UPDATE sync_devices SET trusted=1, name=?2, device_id=?3, \
                 last_seen=datetime('now','localtime') WHERE fingerprint=?1",
                rusqlite::params![fp, name, device_id],
            )
        }
        .unwrap_or(0);
        if updated == 0 {
            let _ = conn.execute(
                "INSERT INTO sync_devices(device_id, name, fingerprint, platform, trusted, last_seen) \
                 VALUES(?1,?2,?3,?4,1,datetime('now','localtime'))",
                rusqlite::params![device_id, name, fp, platform],
            );
        }
    }
}

/// 收到传输请求时是否把窗口显示到前台（设置开关，默认开启）。
fn focus_on_request_enabled() -> bool {
    AppState::open_db_for_sync()
        .and_then(|c| {
            c.query_row(
                "SELECT value FROM settings WHERE key='sync_focus_on_request'",
                [],
                |r| r.get::<_, String>(0),
            )
            .ok()
        })
        .map(|v| v != "0")
        .unwrap_or(true)
}

/// 通知接收方「有人要发东西给你」。
///
/// **不再携带 items 明细**：文件夹可能有几百上千个文件，把整份清单塞进事件，
/// 前端要在弹层入场动画那一帧做反序列化，这就是「收到传输请求弹层卡一下」的来源。
/// 弹层只需要数量/总大小；明细由前端在真正打开接收页时用 `sync_pending_items` 按需拉。
pub fn emit_request(
    id: &str,
    transfer_id: &str,
    from: &str,
    platform: &str,
    fp: &str,
    trusted: bool,
    kind: &str,
    name: &str,
    size: u64,
    count: usize,
) {
    if let Some(app) = AppState::app_handle_for_sync() {
        // 收到请求时把窗口拉到桌面前端（show + 取消最小化 + 置焦），
        // 避免用户在别的窗口错过确认；Windows 前台抢占有系统限制，尽力而为。
        // 移动端没有 WebviewWindow 概念，跳过置前，仅靠事件 + 系统通知提醒。
        let focus = focus_on_request_enabled();
        #[cfg(desktop)]
        if focus {
            if let Some(win) = app.get_webview_window("main") {
                let _ = win.show();
                let _ = win.unminimize();
                let _ = win.set_focus();
            }
        }
        // 移动端无桌面窗口置前概念：发系统通知提醒用户，点击通知会唤起 App，
        // SyncView 挂载后通过 pullPending 补弹窗确认。
        #[cfg(target_os = "android")]
        if focus {
            use tauri_plugin_notification::NotificationExt;
            let body = if count > 1 {
                format!("{} 向你发送了 {} 个文件", from, count)
            } else {
                format!("{} 向你发送了 {}", from, name)
            };
            let _ = app
                .notification()
                .builder()
                .title("RunHelp 收到传输请求")
                .body(body)
                .show();
        }
        let _ = app.emit(
            "sync://request",
            serde_json::json!({
                "id": id,
                "transfer_id": transfer_id,
                "from": from,
                "from_platform": platform,
                "fingerprint": fp,
                "trusted": trusted,
                "kind": kind,
                "name": name,
                "size": size,
                "count": count,
                "items": [],
                "focus": focus,
            }),
        );
    }
}

pub fn emit_received(event: ReceivedEvent) {
    if let Some(app) = AppState::app_handle_for_sync() {
        let _ = app.emit("sync://received", event);
    }
}

fn emit_progress(app: &tauri::AppHandle, e: &ProgressEvent) {
    let _ = app.emit("sync://progress", e);
}

/// 发送状态事件：等待对方接受、对方已接受、已取消等，不落库。
pub fn emit_send_status(app: &tauri::AppHandle, transfer_id: &str, status: &str, sent: u64, total: u64) {
    let _ = app.emit(
        "sync://progress",
        ProgressEvent {
            transfer_id: transfer_id.to_string(),
            sent,
            total,
            status: status.to_string(),
        },
    );
}

/// 后台发送失败事件：命令本身立即返回，错误通过事件告诉弹窗。
pub fn emit_send_error(app: &tauri::AppHandle, transfer_id: &str, error: &str) {
    let _ = app.emit(
        "sync://send-error",
        serde_json::json!({ "transfer_id": transfer_id, "error": error }),
    );
}

/// 发送端撤销请求事件：接收方据此关闭对应请求的弹窗/待接收项
pub fn emit_request_cancelled(transfer_id: &str) {
    if let Some(app) = AppState::app_handle_for_sync() {
        let _ = app.emit(
            "sync://request-cancelled",
            serde_json::json!({ "transfer_id": transfer_id }),
        );
    }
}

/// 接收请求被中止（等待超时 / 发送方掉线）：接收方据此把待接收项标记失效并清理弹窗。
pub fn emit_request_timeout(transfer_id: &str, reason: &str) {
    if let Some(app) = AppState::app_handle_for_sync() {
        let _ = app.emit(
            "sync://request-timeout",
            serde_json::json!({ "transfer_id": transfer_id, "reason": reason }),
        );
    }
}

/// 接收进度事件（供接收方 UI 显示进度条）
pub fn emit_receive_progress(name: &str, received: u64, total: u64, status: &str) {    if let Some(app) = AppState::app_handle_for_sync() {
        let _ = app.emit(
            "sync://receive-progress",
            serde_json::json!({
                "name": name, "received": received, "total": total, "status": status,
            }),
        );
    }
}

/// 记录一条设备上下线诊断日志。
///
/// 两个去处：① `state_log` 环形缓冲（同步页「设备日志」直接看，可滚动/截图）；
/// ② 运行日志面板（`run://log`），便于和设备收发日志放在一起看。
///
/// 排查「设备在离线和在线之间来回切」时，仅凭界面无法知道是谁判的：
/// 广播超时 / 探测连续失败 / 手动刷新一次即判 / BYE / 扫描兜底过期，
/// 这几种成因的处理方式完全不同，所以每行都带原因和两个时间戳的年龄。
pub fn note_state(rt: &SyncRuntime, level: &str, device: &str, line: String) {
    push_state_log(rt, level, device, line, true);
}

/// 只进环形缓冲、不进运行日志面板的明细行（例如「探测未应答 1/3」）。
/// 这类行在设备持续抖动时可能每 2 秒一条，直接灌进日志面板会把有用的上下文冲掉。
pub fn note_state_detail(rt: &SyncRuntime, level: &str, device: &str, line: String) {
    push_state_log(rt, level, device, line, false);
}

fn push_state_log(rt: &SyncRuntime, level: &str, device: &str, line: String, mirror: bool) {
    if let Ok(mut log) = rt.state_log.lock() {
        if log.len() >= super::STATE_LOG_CAP {
            log.pop_front();
        }
        log.push_back(super::StateLogEntry {
            ts: super::now_secs(),
            level: level.to_string(),
            device: device.to_string(),
            line: line.clone(),
        });
    }
    if !mirror {
        return;
    }
    if let Some(app) = AppState::app_handle_for_sync() {
        let _ = app.emit(
            "run://log",
            crate::models::LogEvent {
                // 独立 run_id：前端日志面板按行显示，和项目流水线的 run_id 区分开
                run_id: "sync-device".to_string(),
                stream: if level == "warn" { "stderr" } else { "info" }.to_string(),
                line,
            },
        );
    }
}

/// 读取设备状态诊断日志（最近的在后）。limit 为 0 时返回全部。
#[tauri::command]
pub fn sync_state_log(app: tauri::AppHandle, limit: usize) -> Result<Vec<super::StateLogEntry>, String> {
    let runtime = rt(&app);
    let log = runtime.state_log.lock().map_err(|e| e.to_string())?;
    let take = if limit == 0 { log.len() } else { limit.min(log.len()) };
    Ok(log.iter().skip(log.len() - take).cloned().collect())
}

/// 启动同步服务（发现 + HTTP 接收）
#[tauri::command]
pub fn sync_start(app: tauri::AppHandle) -> Result<(), String> {
    let runtime = rt(&app);
    if runtime.started.load(Ordering::SeqCst) {
        return Ok(());
    }
    // 允许 stop 后 restart：先清除上次的停止标志
    runtime.discovery_stop.store(false, Ordering::SeqCst);
    runtime.server_stop.store(false, Ordering::SeqCst);
    let (device_id, name) = {
        let conn = AppState::open_db_for_sync().ok_or("数据库未就绪")?;
        (super::device_id(&conn), super::display_name(&conn))
    };
    if let Err(e) = transfer::start_server(runtime.clone()) {
        return Err(e);
    }
    discovery::start(
        runtime.clone(),
        device_id.clone(),
        name.clone(),
        std::env::consts::OS.to_string(),
        super::TRANSFER_PORT,
        runtime.identity.fingerprint.clone(),
    )?;
    // 网段主动探测：热点/AP 隔离下 UDP 广播不可达时的发现兜底。
    discovery::start_subnet_scan(runtime.clone(), device_id, name);
    // 近期设备存活探测：只探已知 IP，连续失败即置离线，解决热点下「上线快、离线慢」。
    start_recent_liveness(app.clone());
    runtime.started.store(true, Ordering::SeqCst);
    // 移动端启动前台服务 + WakeLock，避免锁屏/切后台后 UDP 发现与 TCP 监听被冻结。
    // 桌面端是空实现，无副作用。
    let _ = crate::mobile::mobile_start_keepalive(app.clone());
    Ok(())
}

/// 停止同步服务
#[tauri::command]
pub fn sync_stop(app: tauri::AppHandle) -> Result<(), String> {
    let runtime = rt(&app);
    runtime.discovery_stop.store(true, Ordering::SeqCst);
    runtime.server_stop.store(true, Ordering::SeqCst);
    runtime.started.store(false, Ordering::SeqCst);
    // 移动端释放前台服务与锁；桌面端空实现。
    let _ = crate::mobile::mobile_stop_keepalive(app.clone());
    Ok(())
}

/// 同步页状态：设备名、配对码、接收目录、运行态
#[tauri::command]
pub fn sync_status(app: tauri::AppHandle) -> Result<serde_json::Value, String> {
    let runtime = rt(&app);
    let (device_id, name) = {
        let conn = AppState::open_db_for_sync().ok_or("数据库未就绪")?;
        (super::device_id(&conn), super::display_name(&conn))
    };
    Ok(serde_json::json!({
        "device_id": device_id,
        "device_name": name,
        "fingerprint": runtime.identity.fingerprint.clone(),
        "fingerprint_short": runtime.identity.grouped(),
        "receive_dir": receive_dir().to_string_lossy().to_string(),
        "running": runtime.started.load(Ordering::SeqCst),
        "port": super::TRANSFER_PORT,
    }))
}

/// 修改本机设备名（仅同步功能使用）
#[tauri::command]
pub fn sync_set_device_name(name: String) -> Result<(), String> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err("设备名不能为空".to_string());
    }
    let conn = AppState::open_db_for_sync().ok_or("数据库未就绪")?;
    conn.execute(
        "INSERT OR REPLACE INTO settings(key, value) VALUES('device_name', ?1)",
        [&name],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// 设置接收目录
#[tauri::command]
pub fn sync_set_receive_dir(path: String) -> Result<(), String> {
    let conn = AppState::open_db_for_sync().ok_or("数据库未就绪")?;
    conn.execute(
        "INSERT OR REPLACE INTO settings(key, value) VALUES('sync_receive_dir', ?1)",
        [&path],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// 列出发现的设备（合并数据库可信状态）
#[tauri::command]
pub fn sync_list_devices(app: tauri::AppHandle) -> Result<Vec<DeviceInfo>, String> {
    let runtime = rt(&app);
    let now = super::now_secs();
    let mut devices: Vec<DeviceInfo> = runtime
        .devices
        .lock()
        .map_err(|e| e.to_string())?
        .values()
        .cloned()
        // 实时按最后发现时间重算在线状态：关闭软件（含强杀，来不及发 BYE）后
        // 刷新即可立刻反映离线，不必等后台 discovery 线程的超时清理。
        .map(|mut d| {
            d.online = d.is_online(now);
            d
        })
        .collect();

    if let Some(conn) = AppState::open_db_for_sync() {
        for d in &mut devices {
            if d.fingerprint.is_empty() {
                continue;
            }
            if let Ok((trusted, auto, fav, alias, last_ip, db_name, db_platform)) = conn.query_row(
                "SELECT trusted, auto_accept, favorite, alias, last_ip, name, platform FROM sync_devices WHERE fingerprint=?1",
                [&d.fingerprint],
                |r| Ok((
                    r.get::<_, i32>(0)?, r.get::<_, i32>(1)?, r.get::<_, i32>(2)?,
                    r.get::<_, String>(3)?, r.get::<_, String>(4)?, r.get::<_, String>(5)?,
                    r.get::<_, String>(6)?,
                )),
            ) {
                d.trusted = trusted != 0;
                d.auto_accept = auto != 0;
                d.favorite = fav != 0;
                d.alias = alias;
                if !last_ip.is_empty() {
                    d.last_ip = last_ip;
                }
                // 内存里没拿到平台（例如只从数据库补出来的离线记录）时用库里的值补上，
                // 避免列表显示成「unknown / 未知设备」。
                if d.platform.trim().is_empty() || d.platform == "unknown" {
                    if !db_platform.trim().is_empty() && db_platform != "unknown" {
                        d.platform = db_platform.clone();
                    }
                }
                // 设备改名/平台变化后，在线期间把发现到的新信息回写数据库，
                // 否则离线后列表从 sync_devices 表补记录时会显示旧名字/旧平台。
                // 只在值真正变化时写，alias（用户自定义别名）不受影响。
                if d.online && !d.name.is_empty() && db_name != d.name {
                    let _ = conn.execute(
                        "UPDATE sync_devices SET name=?1, last_ip=?2 WHERE fingerprint=?3",
                        [&d.name, &d.ip, &d.fingerprint],
                    );
                }
                if d.online && !d.platform.trim().is_empty() && d.platform != "unknown"
                    && db_platform != d.platform
                {
                    let _ = conn.execute(
                        "UPDATE sync_devices SET platform=?1 WHERE fingerprint=?2",
                        rusqlite::params![&d.platform, &d.fingerprint],
                    );
                }
            }
        }
    }
    // 合并最近连接过的设备（离线常驻），让「近期设备」始终可见且状态即时可判
    if let Some(conn) = AppState::open_db_for_sync() {
        if let Ok(mut stmt) = conn.prepare("SELECT device_id, name, fingerprint, last_ip, trusted, auto_accept, favorite, alias, platform FROM sync_devices ORDER BY last_seen DESC LIMIT 10") {
            if let Ok(rows) = stmt.query_map([], |r| Ok((
                r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?,
                r.get::<_, String>(3)?, r.get::<_, i32>(4)?, r.get::<_, i32>(5)?,
                r.get::<_, i32>(6)?, r.get::<_, String>(7)?, r.get::<_, String>(8)?,
            ))) {
                for row in rows.flatten() {
                    let (id, name, fp, last_ip, trusted, auto, fav, alias, platform) = row;
                    if fp.is_empty() || devices.iter().any(|d| d.fingerprint == fp) {
                        continue;
                    }
                    devices.push(DeviceInfo {
                        id, name, platform: if platform == "unknown" { String::new() } else { platform },
                        ip: last_ip.clone(), port: super::TRANSFER_PORT,
                        fingerprint: fp, last_seen: 0, last_probe: 0,
                        trusted: trusted != 0, auto_accept: auto != 0,
                        favorite: fav != 0, alias, last_ip, online: false, probe_only: false,
                    });
                }
            }
        }
    }
    // 收藏优先、其次在线、再次最近发现
    devices.sort_by(|a, b| {
        b.favorite.cmp(&a.favorite)
            .then(b.online.cmp(&a.online))
            .then(b.last_seen.cmp(&a.last_seen))
    });
    Ok(devices)
}

/// 接收页手动刷新：读取当前内存中的待确认请求（不落库，不打断自动刷新）。
/// 明细不在列表里返回，见 `sync_pending_items`。
#[tauri::command]
pub fn sync_list_pending(app: tauri::AppHandle) -> Result<Vec<serde_json::Value>, String> {
    let holder = rt(&app);
    let pending = holder.pending.lock().map_err(|e| e.to_string())?;
    Ok(pending.values().map(|p| serde_json::json!({
        "id": p.id,
        "transfer_id": p.transfer_id,
        "from": p.from_name,
        "from_platform": p.from_platform,
        "fingerprint": p.from_fp,
        "trusted": is_trusted_fingerprint(&p.from_fp),
        "kind": p.kind,
        "name": p.name,
        "size": p.size,
        "count": p.count,
        "items": [],
    })).collect())
}

/// 按需拉取某个待接收请求的文件明细。
///
/// 请求事件只带数量：文件夹几百上千项时，把清单塞进事件会让弹层入场那一帧
/// 卡住（JSON 反序列化）。前端在真正打开接收页时才调这里。
#[tauri::command]
pub fn sync_pending_items(app: tauri::AppHandle, transfer_id: String) -> Result<Vec<super::PendingItem>, String> {
    let holder = rt(&app);
    let pending = holder.pending.lock().map_err(|e| e.to_string())?;
    Ok(pending
        .values()
        .find(|p| p.transfer_id == transfer_id)
        .map(|p| p.items.clone())
        .unwrap_or_default())
}

/// 手动添加设备：填 IP，探测拿到指纹与设备信息后加入附近列表（信任仍需 TOFU 确认）。
#[tauri::command]
pub fn sync_pair_device(app: tauri::AppHandle, ip: String) -> Result<DeviceInfo, String> {
    let ip = ip.trim().to_string();
    let runtime = rt(&app);
    let identity = runtime.identity.clone();
    let (self_id, self_name) = {
        let conn = AppState::open_db_for_sync();
        let sid = conn.as_ref().map(|c| super::device_id(c)).unwrap_or_default();
        let sname = conn.as_ref().map(|c| super::display_name(c)).unwrap_or_default();
        (sid, sname)
    };
    let probed = transfer::probe(&ip, super::TRANSFER_PORT, &identity, &self_id, &self_name)?;
    if probed.id.is_empty() || probed.fingerprint.is_empty() {
        return Err("目标不是 RunHelp 设备或端口不可达".to_string());
    }
    let id = probed.id.clone();
    let info = DeviceInfo {
        id: id.clone(),
        name: if probed.name.is_empty() { "未知设备".to_string() } else { probed.name },
        // 平台必须用 /hello 返回的对方平台，之前错误地填了本机 OS，
        // 导致手动添加的设备类型显示成自己这台机器的系统。
        platform: if probed.platform.trim().is_empty() { "unknown".to_string() } else { probed.platform },
        ip: ip.clone(),
        port: super::TRANSFER_PORT,
        fingerprint: probed.fingerprint,
        last_seen: super::now_secs(),
        last_probe: super::now_secs(),
        trusted: false,
        auto_accept: false,
        favorite: false,
        alias: String::new(),
        last_ip: ip,
        online: true,
        probe_only: false,
    };
    if let Ok(mut devices) = runtime.devices.lock() {
        devices.insert(id, info.clone());
    }
    Ok(info)
}

/// 收藏/取消收藏设备（按指纹）。收藏即登记身份，便于常用设备突出显示。
#[tauri::command]
pub fn sync_set_favorite(app: tauri::AppHandle, fingerprint: String, favorite: bool) -> Result<(), String> {
    let conn = AppState::open_db_for_sync().ok_or("数据库未就绪")?;
    let id: Option<String> = conn.query_row(
        "SELECT device_id FROM sync_devices WHERE fingerprint=?1", [&fingerprint], |r| r.get(0)
    ).ok();
    if let Some(_id) = id {
        conn.execute("UPDATE sync_devices SET favorite=?1 WHERE fingerprint=?2",
            rusqlite::params![if favorite { 1 } else { 0 }, fingerprint]).map_err(|e| e.to_string())?;
    } else {
        // 尚未入库（仅发现未传输）：用发现列表里的 id/name 建档
        let (dev_id, name, ip) = {
            let runtime = rt(&app);
            let devices = runtime.devices.lock().map_err(|e| e.to_string())?;
            devices.values().find(|d| d.fingerprint == fingerprint)
                .map(|d| (d.id.clone(), d.name.clone(), d.ip.clone()))
                .unwrap_or_else(|| (fingerprint.clone(), String::new(), String::new()))
        };
        conn.execute(
            "INSERT INTO sync_devices(device_id, name, fingerprint, favorite, last_ip, last_seen) \
             VALUES(?1,?2,?3,?4,?5,datetime('now','localtime'))",
            rusqlite::params![dev_id, name, fingerprint, if favorite { 1 } else { 0 }, ip],
        ).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// 修改设备别名（按指纹）
#[tauri::command]
pub fn sync_set_alias(fingerprint: String, alias: String) -> Result<(), String> {
    let conn = AppState::open_db_for_sync().ok_or("数据库未就绪")?;
    conn.execute("UPDATE sync_devices SET alias=?1 WHERE fingerprint=?2",
        rusqlite::params![alias, fingerprint]).map_err(|e| e.to_string())?;
    Ok(())
}

/// 设置设备自动接收（按指纹）
#[tauri::command]
pub fn sync_set_auto_accept(fingerprint: String, enabled: bool) -> Result<(), String> {
    let conn = AppState::open_db_for_sync().ok_or("数据库未就绪")?;
    conn.execute(
        "UPDATE sync_devices SET auto_accept=?1 WHERE fingerprint=?2",
        rusqlite::params![if enabled { 1 } else { 0 }, fingerprint],
    )
    .map_err(|e| e.to_string())?;
    if enabled {
        if let Some(holder) = AppState::sync_holder_for_sync() {
            if let Ok(mut fps) = holder.trusted_fps.lock() {
                fps.insert(fingerprint);
            }
        }
    }
    Ok(())
}

/// 删除设备（按指纹）
#[tauri::command]
pub fn sync_remove_device(fingerprint: String) -> Result<(), String> {
    let conn = AppState::open_db_for_sync().ok_or("数据库未就绪")?;
    conn.execute("DELETE FROM sync_devices WHERE fingerprint=?1", [&fingerprint])
        .map_err(|e| e.to_string())?;
    if let Some(holder) = AppState::sync_holder_for_sync() {
        if let Ok(mut fps) = holder.trusted_fps.lock() {
            fps.remove(&fingerprint);
        }
        if let Ok(mut devices) = holder.devices.lock() {
            devices.retain(|_, d| d.fingerprint != fingerprint);
        }
    }
    Ok(())
}

fn respond_request(request_id: &str, decision: String) -> Result<(), String> {
    let holder = AppState::sync_holder_for_sync().ok_or("同步服务未就绪")?;
    let responded = {
        let pending = holder.pending.lock().map_err(|e| e.to_string())?;
        match pending.get(request_id) {
            Some(req) => {
                let _ = req.respond.send(decision);
                true
            }
            None => false,
        }
    };
    if responded {
        Ok(())
    } else {
        Err("请求已超时或已被撤销".to_string())
    }
}

/// 响应接收确认（前端弹窗点允许/拒绝）
#[tauri::command]
pub fn sync_respond(request_id: String, accepted: bool) -> Result<(), String> {
    respond_request(&request_id, if accepted { "all".to_string() } else { "none".to_string() })
}

/// 部分接收：indices 为接收项索引；空数组表示全部拒绝。
#[tauri::command]
pub fn sync_respond_selected(request_id: String, indices: Vec<usize>) -> Result<(), String> {
    let decision = if indices.is_empty() {
        "none".to_string()
    } else {
        indices.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(",")
    };
    respond_request(&request_id, decision)
}

/// 发送端取消：通过网络通知对端撤销本次传输（弹窗数据只存在于内存）。
#[tauri::command]
pub fn sync_cancel_send(app: tauri::AppHandle, ip: String, transfer_id: String) -> Result<(), String> {
    let runtime = rt(&app);
    if let Ok(sends) = runtime.active_sends.lock() {
        if let Some(flag) = sends.get(&transfer_id) {
            flag.store(true, Ordering::SeqCst);
        }
    }
    // 先更新 UI，通知对端放后台做，避免取消按钮本身卡住。
    emit_send_status(&app, &transfer_id, "cancelled", 0, 0);
    let app_bg = app.clone();
    let runtime_bg = runtime.clone();
    let bg_id = transfer_id.clone();
    std::thread::Builder::new()
        .name("runhelp-sync-cancel".to_string())
        .spawn(move || {
            let _ = transfer::send_cancel(&ip, super::TRANSFER_PORT, &runtime_bg.identity, &bg_id);
            let _ = &app_bg;
        })
        .map_err(|e| format!("启动撤销线程失败: {}", e))?;
    Ok(())
}

/// 发送文字 / 剪贴板内容到指定设备。
/// 注意：传输在后台线程执行，命令本身立即返回，避免等待对方接收时卡住发送弹窗。
#[tauri::command]
pub fn sync_send_text(
    app: tauri::AppHandle,
    ip: String,
    text: String,
    kind: String,
    transfer_id: String,
) -> Result<(), String> {
    let runtime = rt(&app);
    emit_send_status(&app, &transfer_id, "waiting", 0, 1);
    let app_bg = app.clone();
    let runtime_bg = runtime.clone();
    let bg_text = text.clone();
    let bg_kind = kind.clone();
    let bg_id = transfer_id.clone();

    std::thread::Builder::new()
        .name("runhelp-sync-send-text".to_string())
        .spawn(move || {
            let result = (|| -> Result<(), String> {
                let (from_id, from_name) = {
                    let conn = AppState::open_db_for_sync().ok_or("数据库未就绪")?;
                    (super::device_id(&conn), super::display_name(&conn))
                };
                transfer::send_text(
                    &ip,
                    super::TRANSFER_PORT,
                    &runtime_bg.identity,
                    &from_id,
                    &from_name,
                    &bg_kind,
                    &bg_text,
                    &bg_id,
                )
            })();
            match result {
                Ok(()) => {
                    emit_send_status(&app_bg, &bg_id, "accepted", 0, 1);
                    emit_send_status(&app_bg, &bg_id, "done", bg_text.len() as u64, bg_text.len() as u64);
                    record_transfer("send", &bg_kind, &bg_text.chars().take(40).collect::<String>(), bg_text.len() as u64, "success", "");
                }
                Err(e) => {
                    emit_send_error(&app_bg, &bg_id, &e);
                    record_transfer("send", &bg_kind, &bg_text.chars().take(40).collect::<String>(), 0, "failed", &e);
                }
            }
        })
        .map_err(|e| format!("启动发送线程失败: {}", e))?;
    Ok(())
}

/// 发送文件或文件夹（paths 为本地绝对路径，目录递归展开）。
/// 同上：清单等待与文件传输都在后台线程，前端弹窗保持可点击取消。
#[tauri::command]
pub fn sync_send_files(
    app: tauri::AppHandle,
    ip: String,
    paths: Vec<String>,
    names: Option<Vec<String>>,
    transfer_id: String,
) -> Result<(), String> {
    let runtime = rt(&app);
    let cancel_flag = Arc::new(AtomicBool::new(false));
    if let Ok(mut sends) = runtime.active_sends.lock() {
        sends.insert(transfer_id.clone(), cancel_flag.clone());
    }
    emit_send_status(&app, &transfer_id, "waiting", 0, paths.len() as u64);

    let app_bg = app.clone();
    let runtime_bg = runtime.clone();
    let bg_id = transfer_id.clone();
    let bg_flag = cancel_flag.clone();

    std::thread::Builder::new()
        .name("runhelp-sync-send-files".to_string())
        .spawn(move || {
            let result = (|| -> Result<Option<(Vec<(String, PathBuf)>, Vec<usize>, u64)>, String> {
                let (from_id, from_name) = {
                    let conn = AppState::open_db_for_sync().ok_or("数据库未就绪")?;
                    (super::device_id(&conn), super::display_name(&conn))
                };
                // 移动端的 paths 是 content:// 复制到 cacheDir 后的本地副本，文件名可能被
                // uniqueFile 加了 _1/_2 后缀；真正要展示、落盘的名字由前端通过 names 传进来。
                // 这里必须用同一个名字做「清单 + 传输 rel path」，否则接收端按名字匹配不上清单项，
                // 表现为「发送方显示完成、接收方永远卡在接收中」（实际文件已经落盘）。
                let display_names = names.unwrap_or_default();
                let use_display_names = !display_names.is_empty() && display_names.len() == paths.len();
                let mut files: Vec<(String, PathBuf)> = Vec::new();
                for (idx, p) in paths.iter().enumerate() {
                    let path = PathBuf::from(p);
                    if path.is_dir() {
                        let base = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "folder".to_string());
                        collect_files(&path, &base, &mut files)?;
                    } else if path.is_file() {
                        let base = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "file".to_string());
                        let rel = match display_names.get(idx) {
                            Some(n) if use_display_names && !n.trim().is_empty() => n.trim().to_string(),
                            _ => base,
                        };
                        files.push((rel, path));
                    }
                }
                if files.is_empty() {
                    return Err("没有可发送的文件".to_string());
                }
                // items 直接取 files 的 rel：清单与后续 /file 请求天然同名，不会错位。
                let items: Vec<super::PendingItem> = files
                    .iter()
                    .map(|(rel, path)| super::PendingItem {
                        name: rel.clone(),
                        size: std::fs::metadata(path).map(|m| m.len()).unwrap_or(0),
                    })
                    .collect();

                let selected = transfer::send_manifest(
                    &ip,
                    super::TRANSFER_PORT,
                    &runtime_bg.identity,
                    &from_id,
                    &from_name,
                    &bg_id,
                    &items,
                )?;
                if selected.is_empty() || bg_flag.load(Ordering::SeqCst) {
                    return Ok(None);
                }
                emit_send_status(&app_bg, &bg_id, "accepted", 0, selected.len() as u64);
                let accepted_total: u64 = selected.iter().map(|&i| items.get(i).map(|x| x.size).unwrap_or(0)).sum();
                let chosen: Vec<(String, PathBuf)> = selected
                    .iter()
                    .filter_map(|&i| files.get(i).cloned())
                    .collect();
                // 整批复用一条 TLS 连接发送：以前每个文件一次握手，发几百个文件的文件夹
                // 会把 CPU 占满几秒，前端表现为「状态页持续卡顿」。
                // 进度按 150ms 合并上报：128KB 一块回调很密，大文件每秒上百次会淹没前端。
                let mut last_emit = Instant::now();
                let app_prog = app_bg.clone();
                let id_prog = bg_id.clone();
                let mut on_progress = |sent: u64, _file_sent: u64| {
                    if last_emit.elapsed() >= Duration::from_millis(150) {
                        emit_progress(
                            &app_prog,
                            &ProgressEvent {
                                transfer_id: id_prog.clone(),
                                sent,
                                total: accepted_total,
                                status: "transferring".to_string(),
                            },
                        );
                        last_emit = Instant::now();
                    }
                };
                match transfer::send_files_bulk(
                    &ip,
                    super::TRANSFER_PORT,
                    &runtime_bg.identity,
                    &from_id,
                    &from_name,
                    &chosen,
                    &bg_id,
                    &bg_flag,
                    &mut on_progress,
                ) {
                    Ok(_) => {}
                    Err(e) if e.contains("撤销") || bg_flag.load(Ordering::SeqCst) => {
                        emit_send_status(&app_bg, &bg_id, "cancelled", 0, chosen.len() as u64);
                        return Ok(None);
                    }
                    Err(e) => return Err(e),
                }
                Ok(Some((files, selected, accepted_total)))
            })();

            if let Ok(mut sends) = runtime.active_sends.lock() {
                sends.remove(&bg_id);
            }
            match result {
                Ok(Some((files, selected, total))) => {
                    emit_progress(&app_bg, &ProgressEvent {
                        transfer_id: bg_id.clone(), sent: total, total, status: "done".to_string(),
                    });
                    record_transfer("send", "file", &files[selected[0]].0, total, "success", "");
                }
                Ok(None) => {
                    emit_send_status(&app_bg, &bg_id, "cancelled", 0, paths.len() as u64);
                    let name = paths.first().map(|p| PathBuf::from(p).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "file".to_string())).unwrap_or_else(|| "file".to_string());
                    record_transfer("send", "file", &name, 0, "cancelled", "对方未接收或发送端已撤销");
                }
                Err(e) => {
                    emit_send_error(&app_bg, &bg_id, &e);
                    let name = paths.first().map(|p| PathBuf::from(p).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "file".to_string())).unwrap_or_else(|| "file".to_string());
                    record_transfer("send", "file", &name, 0, "failed", &e);
                }
            }
        })
        .map_err(|e| format!("启动发送线程失败: {}", e))?;
    Ok(())
}

fn collect_files(dir: &std::path::Path, prefix: &str, out: &mut Vec<(String, PathBuf)>) -> Result<(), String> {
    let entries = std::fs::read_dir(dir).map_err(|e| e.to_string())?;
    let mut entries: Vec<_> = entries.filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        let rel = format!("{}/{}", prefix, name);
        if path.is_dir() {
            collect_files(&path, &rel, out)?;
        } else if path.is_file() {
            out.push((rel.clone(), path));
        }
    }
    Ok(())
}

/// 接收端落库（供 HTTP 线程在文件/文字接收成功后调用）
pub fn record_receive(kind: &str, name: &str, from: &str, size: u64) {
    if let Some(conn) = AppState::open_db_for_sync() {
        let _ = conn.execute(
            "INSERT INTO sync_transfers(direction, peer_name, kind, item_name, size, status) \
             VALUES('receive', ?1, ?2, ?3, ?4, 'success')",
            rusqlite::params![from, kind, name, size as i64],
        );
    }
}

fn record_transfer(direction: &str, kind: &str, name: &str, size: u64, status: &str, error: &str) {
    if let Some(conn) = AppState::open_db_for_sync() {
        let _ = conn.execute(
            "INSERT INTO sync_transfers(direction, kind, item_name, size, status, error) VALUES(?1,?2,?3,?4,?5,?6)",
            rusqlite::params![direction, kind, name, size as i64, status, error],
        );
    }
}

/// 后台存活探测：同步服务启动后每 2 秒探测一次近期设备。
/// 只访问已知 IP，比 /24 全段扫描轻得多；状态真正变化时才通知前端刷新。
///
/// 这个线程同时承担「发现层的变更广播」：UDP 发现线程没有 AppHandle，
/// 它只在设备新增/由离线转在线/超时被清掉时把 `device_dirty` 置位，
/// 由这里统一 emit `sync://devices-updated`，于是上线也是 1~2 秒级，不靠前端轮询。
pub fn start_recent_liveness(app: tauri::AppHandle) {
    let runtime = rt(&app);
    std::thread::Builder::new()
        .name("runhelp-sync-liveness".to_string())
        .spawn(move || loop {
            if runtime.discovery_stop.load(Ordering::SeqCst) {
                break;
            }
            // 分段 sleep：停止同步时能立刻退出，不必等满一个周期。
            // 每 500ms 检查一次发现层标志，让新设备尽快「亮」起来。
            for _ in 0..(super::LIVENESS_INTERVAL_SECS * 2) {
                if runtime.discovery_stop.load(Ordering::SeqCst) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(500));
                if runtime.device_dirty.swap(false, Ordering::SeqCst) {
                    let _ = app.emit("sync://devices-updated", ());
                }
            }
            if runtime.started.load(Ordering::SeqCst) {
                probe_recent_async(&app, 10, false, false);
            }
        })
        .ok();
}

/// `probe_inflight` 的复位守卫：探测线程无论正常结束、提前 return 还是 panic，
/// 都必须把标志放开。曾经只在函数末尾 store(false)，一次异常就会把标志永久留在 true，
/// 之后所有探测都被 `compare_exchange` 挡掉，设备状态从此停住不动。
struct InflightGuard(Arc<SyncRuntime>);

impl Drop for InflightGuard {
    fn drop(&mut self) {
        self.0.probe_inflight.store(false, Ordering::SeqCst);
    }
}

/// 主动探测最近连接过的设备（最多 limit 台），立即判定在线/离线。
/// 探测放到后台线程异步执行，command 立即返回候选数量，绝不阻塞调用线程/IPC worker。
/// 手动/页面打开调用 notify_always=true；后台存活探测用 false，仅在状态变化时发事件。
/// `strict`：手动刷新时启用——一次不应答就判离线（LocalSend 的手动刷新语义：
/// 「没应答的设备就是不可达」，用户点了刷新要立刻看到真实结果，而不是等三次失败计数）。
/// 手动刷新用更宽的握手超时（probe_manual），避免弱信号下一次超时就误判。
fn probe_recent_async(app: &tauri::AppHandle, limit: usize, notify_always: bool, strict: bool) -> usize {
    let runtime = rt(app);
    // 页面打开、手动刷新、后台存活探测可能同时触发；同一时刻只跑一轮，避免重复握手。
    if runtime.probe_inflight
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        // 手动刷新（页面打开 / 下拉刷新）正好撞上后台探测时，不能直接把这次刷新丢掉：
        // 等上一轮结束（≤1 秒）立刻补一轮，否则用户点了刷新却看不到任何变化。
        if notify_always {
            let app2 = app.clone();
            let rt2 = runtime.clone();
            std::thread::spawn(move || {
                for _ in 0..20 {
                    std::thread::sleep(Duration::from_millis(100));
                    if !rt2.probe_inflight.load(Ordering::SeqCst) {
                        probe_recent_async(&app2, limit, true, strict);
                        return;
                    }
                }
                let _ = app2.emit("sync://devices-updated", ());
            });
        }
        return 0;
    }
    let mut candidates: Vec<(String, String, String, String)> = {
        let Some(conn) = AppState::open_db_for_sync() else {
            runtime.probe_inflight.store(false, Ordering::SeqCst);
            return 0;
        };
        let Ok(mut stmt) = conn.prepare(
            "SELECT device_id, name, fingerprint, last_ip FROM sync_devices \
             WHERE last_ip != '' ORDER BY last_seen DESC LIMIT ?1"
        ) else {
            runtime.probe_inflight.store(false, Ordering::SeqCst);
            return 0;
        };
        let Ok(rows) = stmt.query_map([limit as i64], |r| Ok((
            r.get::<_, String>(0)?, r.get::<_, String>(1)?,
            r.get::<_, String>(2)?, r.get::<_, String>(3)?,
        ))) else {
            runtime.probe_inflight.store(false, Ordering::SeqCst);
            return 0;
        };
        rows.filter_map(|r| r.ok()).filter(|(_, _, _, ip)| !ip.is_empty()).collect()
    };
    // 广播发现但还没入库的设备（第一次见、没建立信任）也要探活，
    // 否则它们只能等广播超时，离线感知明显比上线慢。
    // 仅靠网段扫描发现的设备（probe_only）**必须**一起探：
    // 它们没有广播心跳，以前被跳过，只能靠 15 秒一次的扫描续命，
    // 而兜底超时是 12 秒 —— 于是每 15 秒必然闪一次「离线」再自己回来。
    if let Ok(devices) = runtime.devices.lock() {
        for d in devices.values() {
            if d.ip.is_empty() {
                continue;
            }
            if candidates
                .iter()
                .any(|(_, _, fp, ip)| fp == &d.fingerprint || ip == &d.ip)
            {
                continue;
            }
            candidates.push((d.id.clone(), d.name.clone(), d.fingerprint.clone(), d.ip.clone()));
        }
    }
    let count = candidates.len();
    if candidates.is_empty() {
        runtime.probe_inflight.store(false, Ordering::SeqCst);
        return 0;
    }
    let (self_id, self_name) = {
        let conn = AppState::open_db_for_sync();
        (
            conn.as_ref().map(|c| super::device_id(c)).unwrap_or_default(),
            conn.as_ref().map(|c| super::display_name(c)).unwrap_or_default(),
        )
    };
    let app_bg = app.clone();
    let changed = Arc::new(AtomicBool::new(false));
    // 关键：spawn 后立即返回；join 全在后台线程内完成，不占用同步 command 的 worker。
    std::thread::spawn(move || {
        // 无论怎么退出（包括 panic）都复位探测锁，避免一次异常把探测永久卡死。
        let _inflight = InflightGuard(runtime.clone());
        let identity = runtime.identity.clone();
        let handles: Vec<_> = candidates
            .into_iter()
            .map(|(id, name, fp, ip)| {
                let rt = runtime.clone();
                let identity = identity.clone();
                let self_id = self_id.clone();
                let self_name = self_name.clone();
                let changed = changed.clone();
                let app_dev = app_bg.clone();
                std::thread::spawn(move || {
                    // 单台设备的结果立刻落到运行时并**立刻推事件**。
                    // 之前要等整轮 join 之后才重算并发事件：只要列表里有一台手机被系统冻结
                    // （TCP 连得上、应用层不应答），整轮就被它拖住，其它设备的上下线也一起延迟。
                    let started = Instant::now();
                    let mut flipped = false;
                    // 失败计数按设备 id 记：指纹可能为空（旧版本对端），
                    // 用空串当 key 会让所有这类设备共用同一个计数器，一台失败把另一台也带成离线。
                    let miss_key = if !id.is_empty() {
                        id.clone()
                    } else if !fp.is_empty() {
                        fp.clone()
                    } else {
                        ip.clone()
                    };
                    // 手动刷新用更宽的超时：用户点一下应该得到「真实但别误判」的结果，
                    // 600ms/900ms 是给 2 秒一轮的后台探测省射频的，手动场景等 1.2s 完全可接受。
                    let probe_result = if strict {
                        transfer::probe_manual(&ip, super::TRANSFER_PORT, &identity, &self_id, &self_name)
                    } else {
                        transfer::probe_liveness(&ip, super::TRANSFER_PORT, &identity, &self_id, &self_name)
                    };
                    match probe_result {
                        Ok(info) => {
                            let rtt = started.elapsed().as_millis() as u64;
                            let use_id = if info.id.is_empty() { id } else { info.id };
                            let use_fp = if info.fingerprint.is_empty() { fp } else { info.fingerprint };
                            // 探测成功即清零失败计数；原本离线的设备要通知前端立刻刷新。
                            if let Ok(mut misses) = rt.probe_misses.lock() {
                                misses.remove(&miss_key);
                                misses.remove(&use_fp);
                            }
                            let was_online = rt.devices.lock().ok()
                                .and_then(|devices| devices.values()
                                    .find(|d| (!use_fp.is_empty() && d.fingerprint == use_fp) || d.ip == ip)
                                    .map(|d| d.online))
                                .unwrap_or(false);
                            let outcome = discovery::upsert_device_probe(
                                &rt, use_id,
                                if info.name.is_empty() { name } else { info.name },
                                if info.platform.trim().is_empty() { "unknown".to_string() } else { info.platform },
                                ip.clone(), super::TRANSFER_PORT, use_fp,
                            );
                            if !was_online {
                                flipped = true;
                                // 上线原因写清楚：探测成功说明「应用层应答了」，比 TCP 连通更可信
                                note_state(
                                    &rt, "info", &outcome.name,
                                    format!("[设备] {} 上线 · 探测成功 {}ms · ip {}",
                                        outcome.name, rtt, outcome.ip),
                                );
                            }
                        }
                        Err(err) => {
                            // 判定前先留现场：界面是否还显示在线、两个时间戳各自多久没刷新。
                            // 这是区分「信号真丢了」和「判据写错了」的唯一线索。
                            let now = super::now_secs();
                            let snapshot = rt.devices.lock().ok().and_then(|devices| {
                                devices.values()
                                    .find(|d| {
                                        (!fp.is_empty() && d.fingerprint == fp)
                                            || d.last_ip == ip || d.ip == ip
                                    })
                                    .map(|d| (d.name.clone(), d.online, d.last_seen, d.last_probe))
                            });
                            let (dev_name, was_online, hb_ts, probe_ts) = snapshot
                                .unwrap_or((name.clone(), false, 0, 0));
                            let hb_desc = super::age_desc(now, hb_ts);
                            let probe_desc = super::age_desc(now, probe_ts);
                            // 连续失败达到阈值才判离线，避免单次握手抖动造成设备闪断。
                            let mut misses_now = 0u8;
                            let mut mark_offline = false;
                            if strict {
                                // 手动刷新：不应答即离线，追求「点一下就是真实状态」。
                                if let Ok(mut misses) = rt.probe_misses.lock() {
                                    misses.remove(&miss_key);
                                }
                                misses_now = super::PROBE_OFFLINE_MISSES;
                                mark_offline = true;
                            } else if let Ok(mut misses) = rt.probe_misses.lock() {
                                let n = misses.entry(miss_key.clone()).or_insert(0);
                                *n = n.saturating_add(1);
                                misses_now = *n;
                                if *n >= super::PROBE_OFFLINE_MISSES {
                                    *n = 0;
                                    mark_offline = true;
                                }
                            }
                            // 只对「界面还显示在线」的失败记一行：真正离线的设备每 2 秒都会失败，
                            // 全记下来会把有价值的上下文冲掉。
                            if was_online && !mark_offline {
                                note_state_detail(
                                    &rt, "warn", &dev_name,
                                    format!(
                                        "[设备] {} 探测未应答 {}/{} · {} · 心跳 {} / 探测 {}",
                                        dev_name, misses_now, super::PROBE_OFFLINE_MISSES, err, hb_desc, probe_desc
                                    ),
                                );
                            }
                            if mark_offline {
                                let reason = if strict {
                                    format!("手动刷新探测未应答，一次即判离线（{}）", err)
                                } else {
                                    format!("连续 {} 次探测失败（{}）", super::PROBE_OFFLINE_MISSES, err)
                                };
                                let mut offline_line: Option<String> = None;
                                if let Ok(mut devices) = rt.devices.lock() {
                                    if let Some(d) = devices.values_mut().find(|d| {
                                        (!fp.is_empty() && d.fingerprint == fp)
                                            || d.last_ip == ip || d.ip == ip
                                    }) {
                                        // 两个时间戳都要清：只要有一个还新鲜就仍算在线。
                                        // 广播心跳仍在时不动它（交给 DEVICE_TIMEOUT_SECS 处理）。
                                        let heartbeat_fresh = d.last_seen > 0
                                            && now - d.last_seen < super::DEVICE_TIMEOUT_SECS;
                                        if d.online && !heartbeat_fresh {
                                            d.online = false;
                                            d.last_seen = 0;
                                            d.last_probe = 0;
                                            flipped = true;
                                            offline_line = Some(format!(
                                                "[设备] {} 离线 · {} · 心跳 {} / 探测 {} · ip {}",
                                                d.name, reason, hb_desc, probe_desc, d.ip
                                            ));
                                        }
                                    }
                                }
                                if let Some(line) = offline_line {
                                    note_state(&rt, "warn", &dev_name, line);
                                }
                            }
                        }
                    }
                    if flipped {
                        changed.store(true, Ordering::SeqCst);
                        let _ = app_dev.emit("sync://devices-updated", ());
                    }
                })
            })
            .collect();
        for h in handles {
            let _ = h.join();
        }
        // 整轮结束后再按时间戳扫一遍：广播停了的设备即使探测还没轮到，
        // 也能在本周期内反映到界面，而不必等失败计数攒够。
        let state_changed = {
            let now = super::now_secs();
            let mut flipped = false;
            let mut went_offline: Vec<(String, String, String, String, bool)> = Vec::new();
            if let Ok(mut devices) = runtime.devices.lock() {
                for d in devices.values_mut() {
                    let online = d.is_online(now);
                    if online != d.online {
                        d.online = online;
                        flipped = true;
                        if !online {
                            went_offline.push((
                                d.name.clone(),
                                d.ip.clone(),
                                super::age_desc(now, d.last_seen),
                                super::age_desc(now, d.last_probe),
                                d.probe_only,
                            ));
                        }
                    }
                }
            }
            for (name, ip, hb, pr, probe_only) in went_offline {
                note_state(
                    &runtime, "warn", &name,
                    format!(
                        "[设备] {} 离线 · 广播与探测时间戳都过期（{}）· 心跳 {} / 探测 {} · ip {}",
                        name,
                        if probe_only { "仅扫描发现" } else { "常规" },
                        hb,
                        pr,
                        ip
                    ),
                );
            }
            flipped
        };
        if notify_always || changed.load(Ordering::SeqCst) || state_changed {
            let _ = app_bg.emit("sync://devices-updated", ());
        }
    });
    count
}

/// 主动探测最近连接过的设备（Tauri 命令入口）。
/// `strict`：手动下拉刷新传 true（一次不应答即判离线）；页面打开传 false，
/// 否则「一进同步页」就用了最激进的判据，弱信号下一次 600ms 超时就把设备判成离线，
/// 2 秒后后台探测又成功 —— 用户看到的就是「进页面先闪一下」。
#[tauri::command]
pub fn sync_probe_recent(app: tauri::AppHandle, limit: usize, strict: Option<bool>) -> Result<usize, String> {
    Ok(probe_recent_async(&app, limit, true, strict.unwrap_or(false)))
}

/// 把同步结果写入运行日志（桌面端日志面板可见）。
/// 由前端在成功/失败/拒绝时调用，保证日志包含双方名称、类型和内容简述。
#[tauri::command]
pub fn sync_log_result(app: tauri::AppHandle, payload: serde_json::Value) -> Result<(), String> {
    let from = payload.get("from").and_then(|v| v.as_str()).unwrap_or("本机");
    let to = payload.get("to").and_then(|v| v.as_str()).unwrap_or("未知设备");
    let kind = payload.get("kind").and_then(|v| v.as_str()).unwrap_or("");
    let content = payload.get("content").and_then(|v| v.as_str()).unwrap_or("");
    let result = payload.get("result").and_then(|v| v.as_str()).unwrap_or("");
    let detail = payload.get("detail").and_then(|v| v.as_str()).unwrap_or("");
    let line = format!(
        "[数据同步] {}(发送方) → {}(接收方) · {} · {} · {}{}",
        from, to, result, kind, content,
        if detail.is_empty() { String::new() } else { format!(" · {}", detail) }
    );
    let stream = if result == "失败" || result == "拒绝" { "stderr" } else { "info" };
    let _ = app.emit("run://log", crate::models::LogEvent {
        run_id: "sync".to_string(),
        stream: stream.to_string(),
        line,
    });
    Ok(())
}

/// 传输历史
#[tauri::command]
pub fn sync_list_history(limit: i32) -> Result<Vec<serde_json::Value>, String> {
    let conn = AppState::open_db_for_sync().ok_or("数据库未就绪")?;
    let mut stmt = conn
        .prepare("SELECT id, direction, peer_name, kind, item_name, size, status, error, created_at \
                  FROM sync_transfers ORDER BY id DESC LIMIT ?1")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([limit], |r| {
            Ok(serde_json::json!({
                "id": r.get::<_, i64>(0)?,
                "direction": r.get::<_, String>(1)?,
                "peer": r.get::<_, String>(2)?,
                "kind": r.get::<_, String>(3)?,
                "name": r.get::<_, String>(4)?,
                "size": r.get::<_, i64>(5)?,
                "status": r.get::<_, String>(6)?,
                "error": r.get::<_, String>(7)?,
                "created_at": r.get::<_, String>(8)?,
            }))
        })
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

/// 清空传输历史
#[tauri::command]
pub fn sync_clear_history() -> Result<(), String> {
    let conn = AppState::open_db_for_sync().ok_or("数据库未就绪")?;
    conn.execute("DELETE FROM sync_transfers", [])
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// 重新生成本机设备指纹（删除证书，下次启动重建）。注意：会让所有已收藏设备失效。
#[tauri::command]
pub fn sync_regenerate_identity(app: tauri::AppHandle) -> Result<String, String> {
    let data_dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    identity::reset(&data_dir)?;
    // 同时清空本机所有已信任设备的指纹（否则重启后对不上）
    if let Some(conn) = AppState::open_db_for_sync() {
        let _ = conn.execute("UPDATE sync_devices SET trusted=0, favorite=0, fingerprint=''", []);
    }
    if let Some(holder) = AppState::sync_holder_for_sync() {
        if let Ok(mut fps) = holder.trusted_fps.lock() {
            fps.clear();
        }
    }
    Ok("设备身份已重置，请重启应用后重新配对".to_string())
}
