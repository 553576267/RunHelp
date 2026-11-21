use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use tauri::{Manager, State, Theme};
use tauri::window::Color;

use crate::state::AppState;

/// 切换应用主题时同步原生窗口：标题栏明暗 + 窗口背景色，
/// 避免浅色主题下顶部标题栏仍是深色、以及缩放时露出深色底。
#[tauri::command]
pub fn set_window_theme(theme: String, app: tauri::AppHandle) -> Result<(), String> {
    let win = app
        .get_webview_window("main")
        .ok_or_else(|| "找不到主窗口".to_string())?;
    let is_light = theme == "light";
    let t = if is_light { Theme::Light } else { Theme::Dark };
    win.set_theme(Some(t)).map_err(|e| e.to_string())?;
    // Color 是 RGBA：深色 #14161A，浅色 #f5f6f8
    let color = if is_light {
        Color(245, 246, 248, 255)
    } else {
        Color(20, 22, 26, 255)
    };
    win.set_background_color(Some(color)).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> Result<HashMap<String, String>, String> {
    let conn = state.conn()?;
    let mut stmt = conn
        .prepare("SELECT key, value FROM settings")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .map_err(|e| e.to_string())?;
    let mut out = HashMap::new();
    for r in rows {
        let (k, v) = r.map_err(|e| e.to_string())?;
        out.insert(k, v);
    }
    Ok(out)
}

#[tauri::command]
pub fn set_setting(key: String, value: String, state: State<'_, AppState>) -> Result<(), String> {
    let conn = state.conn()?;
    conn.execute(
        "INSERT INTO settings(key, value) VALUES (?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        rusqlite::params![key, value],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// 当前数据库文件所在目录。
#[tauri::command]
pub fn data_dir(state: State<'_, AppState>) -> Result<String, String> {
    let parent = state
        .db_path
        .parent()
        .ok_or_else(|| "无法确定数据目录".to_string())?;
    Ok(parent.to_string_lossy().to_string())
}

#[derive(serde::Serialize)]
pub struct DbLocation {
    /// 当前实际使用的数据库文件完整路径
    pub current: String,
}

#[tauri::command]
pub fn get_db_location(state: State<'_, AppState>) -> Result<DbLocation, String> {
    Ok(DbLocation {
        current: state.db_path.to_string_lossy().to_string(),
    })
}

// ───────────────────────── 数据库备份与恢复 ─────────────────────────

/// 默认备份目录：坚果云同步目录下的 RunHelpBackups。
fn default_backup_dir() -> PathBuf {
    // USERPROFILE 在 Windows 恒存在；回退到 HOME。
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_else(|_| "C:\\".to_string());
    PathBuf::from(home).join("Nutstore").join("RunHelpBackups")
}

/// 解析实际备份目录：优先 settings.backup_dir，否则默认坚果云目录。
/// 与 Python 脚本 backup_db.py 的 resolve_backup_dir 保持一致。
fn resolve_backup_dir(state: &AppState) -> PathBuf {
    if let Ok(conn) = state.conn() {
        if let Ok(value) = conn.query_row(
            "SELECT value FROM settings WHERE key='backup_dir'",
            [],
            |r| r.get::<_, String>(0),
        ) {
            if !value.trim().is_empty() {
                return PathBuf::from(value.trim());
            }
        }
    }
    if let Ok(v) = std::env::var("RUNHELP_BACKUP_DIR") {
        if !v.trim().is_empty() {
            return PathBuf::from(v.trim());
        }
    }
    default_backup_dir()
}

#[derive(serde::Serialize)]
pub struct BackupInfo {
    /// 文件名（恢复时传这个）
    pub name: String,
    /// 完整路径
    pub path: String,
    /// 字节大小
    pub size: u64,
    /// 修改时间（本地时间字符串）
    pub modified: String,
    /// 是否由本机备份（文件名含本机机器名）
    pub local: bool,
}

/// 默认备份目录（供前端在未配置时预填输入框）
#[tauri::command]
pub fn get_default_backup_dir() -> Result<String, String> {
    Ok(default_backup_dir().to_string_lossy().to_string())
}

/// 列出备份目录里的 runhelp_*.db 文件，按修改时间倒序。
#[tauri::command]
pub fn list_backups(state: State<'_, AppState>) -> Result<Vec<BackupInfo>, String> {
    let dir = resolve_backup_dir(&state);
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let local_host = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_default();
    let mut files: Vec<BackupInfo> = Vec::new();
    let entries = fs::read_dir(&dir).map_err(|e| format!("读取备份目录失败: {}", e))?;
    for entry in entries.flatten() {
        let p = entry.path();
        if !p.is_file() {
            continue;
        }
        let name = match p.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        // 只认脚本生成的 runhelp_*.db，忽略 -wal/-shm 及其它杂项
        if !name.starts_with("runhelp_") || !name.ends_with(".db") {
            continue;
        }
        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        let modified = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .map(|secs| {
                // 用 SQLite 把 epoch 秒转成可读本地时间，避免引入 chrono
                state
                    .conn()
                    .ok()
                    .and_then(|c| {
                        c.query_row(
                            "SELECT datetime(?1,'unixepoch','localtime')",
                            rusqlite::params![secs],
                            |r| r.get::<_, String>(0),
                        )
                        .ok()
                    })
                    .unwrap_or_else(|| secs.to_string())
            })
            .unwrap_or_default();
        files.push(BackupInfo {
            local: !local_host.is_empty() && name.contains(&local_host),
            name,
            path: p.to_string_lossy().to_string(),
            size: meta.len(),
            modified,
        });
    }
    files.sort_by(|a, b| b.modified.cmp(&a.modified));
    Ok(files)
}

/// 从备份恢复数据库。流程：
/// 1. 安全校验文件名（必须 runhelp_*.db、无路径分隔符、确实在备份目录内）
/// 2. 校验备份文件是可打开的 SQLite（integrity_check）
/// 3. checkpoint 当前库，并把当前库另存为 .before-restore_<时间戳> 保命
/// 4. 用 SQLite backup API 把备份写回当前库（不是直接 fs::copy，避免占用/损坏）
/// 5. 写回后立即调用 db::init_db，把旧结构自动升级到当前版本，无需重启
#[tauri::command]
pub fn restore_backup(name: String, state: State<'_, AppState>) -> Result<String, String> {
    let dir = resolve_backup_dir(&state);
    // 防目录穿越：只接受纯文件名
    if name.contains('/') || name.contains('\\') || name.contains("..") {
        return Err("备份名称非法".into());
    }
    if !name.starts_with("runhelp_") || !name.ends_with(".db") {
        return Err("只可恢复 runhelp_*.db 备份文件".into());
    }
    let backup_path = dir.join(&name);
    if !backup_path.is_file() {
        return Err(format!("备份文件不存在: {}", backup_path.to_string_lossy()));
    }

    // 校验备份完整性（只读打开，避免 WAL 副作用）
    {
        let src = rusqlite::Connection::open_with_flags(
            &backup_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .map_err(|e| format!("无法打开备份: {}", e))?;
        let check: String = src
            .query_row("PRAGMA integrity_check", [], |r| r.get(0))
            .map_err(|e| format!("备份校验失败: {}", e))?;
        if check != "ok" {
            return Err(format!("备份文件已损坏: {}", check));
        }
    }

    let current = &state.db_path;

    // 保命：checkpoint 后把当前库另存一份
    if let Ok(conn) = state.conn() {
        let _ = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
    }
    let stamp = state.local_now().replace([' ', ':', '-'], "");
    let safety = current.with_file_name(format!(
        "{}.before-restore_{}.db",
        current.file_stem().unwrap_or_default().to_string_lossy(),
        stamp
    ));
    fs::copy(current, &safety)
        .map_err(|e| format!("备份当前库失败（已中止恢复）: {}", e))?;

    // 用 SQLite backup API 写回：源=备份，目标=当前库
    {
        let src = rusqlite::Connection::open_with_flags(
            &backup_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .map_err(|e| format!("打开备份失败: {}", e))?;
        let mut dst = rusqlite::Connection::open(current)
            .map_err(|e| format!("打开当前库失败: {}", e))?;
        let backup = rusqlite::backup::Backup::new(&src, &mut dst)
            .map_err(|e| format!("初始化恢复失败: {}", e))?;
        backup
            .run_to_completion(100, std::time::Duration::from_millis(250), None)
            .map_err(|e| format!("恢复写入失败: {}", e))?;
    }

    // 旧备份可能缺少新版本的表/列。写回后立刻跑一次迁移，避免用户不重启就
    // 用旧结构触发 SQL 报错；迁移前会再留一份快照，且迁移本身是幂等的。
    crate::db::init_db(current).map_err(|e| format!("恢复成功，但自动升级结构失败: {}", e))?;

    Ok(format!(
        "已从 [{}] 恢复并自动升级到当前结构。当前库已先备份为 {}。",
        name,
        safety.file_name().unwrap_or_default().to_string_lossy()
    ))
}
