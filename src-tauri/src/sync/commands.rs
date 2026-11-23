//! 数据同步的 Tauri 命令与事件发射。

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

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
    let dir = AppState::open_db_for_sync()
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
        .map(PathBuf::from);
    dir.unwrap_or_else(|| {
        let home = std::env::var("USERPROFILE")
            .or_else(|_| std::env::var("HOME"))
            .unwrap_or_default();
        PathBuf::from(home).join("Downloads").join("RunHelp同步")
    })
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
    if fp.is_empty() {
        return;
    }
    if let Some(conn) = AppState::open_db_for_sync() {
        // 先尝试按指纹更新已有记录
        let updated = conn.execute(
            "UPDATE sync_devices SET trusted=1, name=?2, device_id=?3, last_seen=datetime('now','localtime') WHERE fingerprint=?1",
            rusqlite::params![fp, name, device_id],
        ).unwrap_or(0);
        if updated == 0 {
            let _ = conn.execute(
                "INSERT INTO sync_devices(device_id, name, fingerprint, trusted, last_seen) \
                 VALUES(?1,?2,?3,1,datetime('now','localtime'))",
                rusqlite::params![device_id, name, fp],
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
    items: &[super::PendingItem],
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
        let item_values: Vec<_> = items
            .iter()
            .map(|i| serde_json::json!({ "name": i.name, "size": i.size }))
            .collect();
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
                "items": item_values,
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

/// 接收进度事件（供接收方 UI 显示进度条）
pub fn emit_receive_progress(name: &str, received: u64, total: u64, status: &str) {
    if let Some(app) = AppState::app_handle_for_sync() {
        let _ = app.emit(
            "sync://receive-progress",
            serde_json::json!({
                "name": name, "received": received, "total": total, "status": status,
            }),
        );
    }
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
            d.online = now - d.last_seen < super::DEVICE_TIMEOUT_SECS;
            d
        })
        .collect();

    if let Some(conn) = AppState::open_db_for_sync() {
        for d in &mut devices {
            if d.fingerprint.is_empty() {
                continue;
            }
            if let Ok((trusted, auto, fav, alias, last_ip)) = conn.query_row(
                "SELECT trusted, auto_accept, favorite, alias, last_ip FROM sync_devices WHERE fingerprint=?1",
                [&d.fingerprint],
                |r| Ok((
                    r.get::<_, i32>(0)?, r.get::<_, i32>(1)?, r.get::<_, i32>(2)?,
                    r.get::<_, String>(3)?, r.get::<_, String>(4)?,
                )),
            ) {
                d.trusted = trusted != 0;
                d.auto_accept = auto != 0;
                d.favorite = fav != 0;
                d.alias = alias;
                if !last_ip.is_empty() {
                    d.last_ip = last_ip;
                }
            }
        }
    }
    // 合并数据库里收藏但当前不在线的设备（离线常驻）
    if let Some(conn) = AppState::open_db_for_sync() {
        if let Ok(mut stmt) = conn.prepare("SELECT device_id, name, fingerprint, last_ip, trusted, auto_accept, favorite, alias FROM sync_devices WHERE favorite=1 OR trusted=1") {
            if let Ok(rows) = stmt.query_map([], |r| Ok((
                r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?,
                r.get::<_, String>(3)?, r.get::<_, i32>(4)?, r.get::<_, i32>(5)?,
                r.get::<_, i32>(6)?, r.get::<_, String>(7)?,
            ))) {
                for row in rows.flatten() {
                    let (id, name, fp, last_ip, trusted, auto, fav, alias) = row;
                    if fp.is_empty() || devices.iter().any(|d| d.fingerprint == fp) {
                        continue;
                    }
                    devices.push(DeviceInfo {
                        id, name, platform: String::new(),
                        ip: last_ip.clone(), port: super::TRANSFER_PORT,
                        fingerprint: fp, last_seen: 0,
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
        "items": p.items,
    })).collect())
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
    let (id, name, fp) = transfer::probe(&ip, super::TRANSFER_PORT, &identity, &self_id, &self_name)?;
    if id.is_empty() || fp.is_empty() {
        return Err("目标不是 RunHelp 设备或端口不可达".to_string());
    }
    let info = DeviceInfo {
        id: id.clone(),
        name,
        platform: std::env::consts::OS.to_string(),
        ip: ip.clone(),
        port: super::TRANSFER_PORT,
        fingerprint: fp,
        last_seen: super::now_secs(),
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
                let mut files: Vec<(String, PathBuf)> = Vec::new();
                for p in &paths {
                    let path = PathBuf::from(p);
                    if path.is_dir() {
                        let base = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "folder".to_string());
                        collect_files(&path, &base, &mut files)?;
                    } else if path.is_file() {
                        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "file".to_string());
                        files.push((name, path));
                    }
                }
                if files.is_empty() {
                    return Err("没有可发送的文件".to_string());
                }
                let items: Vec<super::PendingItem> = files
                    .iter()
                    .map(|(name, path)| super::PendingItem {
                        name: name.clone(),
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
                let mut sent = 0u64;
                for &index in &selected {
                    if bg_flag.load(Ordering::SeqCst) {
                        emit_send_status(&app_bg, &bg_id, "cancelled", sent, accepted_total);
                        return Ok(None);
                    }
                    let (rel, path) = match files.get(index) {
                        Some(v) => v,
                        None => continue,
                    };
                    let data = std::fs::read(path).map_err(|e| format!("读取 {} 失败: {}", path.display(), e))?;
                    transfer::send_file(
                        &ip,
                        super::TRANSFER_PORT,
                        &runtime_bg.identity,
                        &from_id,
                        &from_name,
                        rel,
                        &data,
                        files.len(),
                        &bg_id,
                        true,
                    )?;
                    sent += data.len() as u64;
                    emit_progress(&app_bg, &ProgressEvent {
                        transfer_id: bg_id.clone(), sent, total: accepted_total, status: "transferring".to_string(),
                    });
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
