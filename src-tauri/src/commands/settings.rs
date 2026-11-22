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
    let lang = app
        .try_state::<AppState>()
        .map(|s| crate::i18n::lang(s.inner()))
        .unwrap_or_else(|| "zh-CN".to_string());
    let win = app
        .get_webview_window("main")
        .ok_or_else(|| crate::i18n::tr(&lang, "找不到主窗口"))?;
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
    let lang = crate::i18n::lang(state.inner());
    let parent = state
        .db_path
        .parent()
        .ok_or_else(|| crate::i18n::tr(&lang, "无法确定数据目录"))?;
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

/// 默认备份目录统一由 platform 层提供（坚果云同步目录），
/// 避免 settings / backup / Python 脚本三处各写一套。

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
    crate::platform::default_backup_dir()
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
    Ok(crate::platform::default_backup_dir().to_string_lossy().to_string())
}

/// 当前平台可用的 Shell 选项（值用于落库，标签由 i18n 字典提供）。
#[derive(serde::Serialize)]
pub struct ShellOption {
    pub value: String,
    pub label: String,
}

#[tauri::command]
pub fn get_shell_options(state: State<'_, AppState>) -> Result<Vec<ShellOption>, String> {
    let lang = crate::i18n::lang(&state);
    Ok(crate::platform::available_shells()
        .iter()
        .map(|kind| ShellOption {
            value: kind.key().to_string(),
            label: crate::i18n::tr(&lang, kind.label_key()),
        })
        .collect())
}

/// 默认 Shell 的唯一来源：读设置里的 default_shell，并把不适配当前平台的老值映射过来。
#[tauri::command]
pub fn get_default_shell(state: State<'_, AppState>) -> Result<String, String> {
    let saved = state
        .conn()
        .ok()
        .and_then(|conn| {
            conn.query_row(
                "SELECT value FROM settings WHERE key='default_shell'",
                [],
                |r| r.get::<_, String>(0),
            )
            .ok()
        })
        .unwrap_or_default();
    let shell = if saved.trim().is_empty() {
        crate::platform::default_shell()
    } else {
        crate::platform::normalize_shell(saved.trim())
    };
    Ok(shell.key().to_string())
}

/// 列出备份目录里的 runhelp_*.db 文件，按修改时间倒序。
#[tauri::command]
pub fn list_backups(state: State<'_, AppState>) -> Result<Vec<BackupInfo>, String> {
    let lang = crate::i18n::lang(state.inner());
    let dir = resolve_backup_dir(&state);
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    // 与备份脚本 gen/cleanup 使用同一套机器名规则，避免名字含空格等字符时
    // 「本机」标签判断错误。
    let local_prefix = crate::platform::local_backup_prefix();
    let mut files: Vec<BackupInfo> = Vec::new();
    let entries = fs::read_dir(&dir).map_err(|e| crate::i18n::trf(&lang, "读取备份目录失败: {error}", &[("error", &e.to_string())]))?;
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
            local: name.starts_with(&local_prefix),
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
    let lang = crate::i18n::lang(state.inner());
    let dir = resolve_backup_dir(&state);
    // 防目录穿越：只接受纯文件名
    if name.contains('/') || name.contains('\\') || name.contains("..") {
        return Err(crate::i18n::tr(&lang, "备份名称非法"));
    }
    if !name.starts_with("runhelp_") || !name.ends_with(".db") {
        return Err(crate::i18n::tr(&lang, "只可恢复 runhelp_*.db 备份文件"));
    }
    let backup_path = dir.join(&name);
    if !backup_path.is_file() {
        return Err(crate::i18n::trf(
            &lang,
            "备份文件不存在: {path}",
            &[("path", &backup_path.to_string_lossy().to_string())],
        ));
    }

    // 校验备份完整性（只读打开，避免 WAL 副作用）
    {
        let src = rusqlite::Connection::open_with_flags(
            &backup_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .map_err(|e| crate::i18n::trf(&lang, "无法打开备份: {error}", &[("error", &e.to_string())]))?;
        let check: String = src
            .query_row("PRAGMA integrity_check", [], |r| r.get(0))
            .map_err(|e| crate::i18n::trf(&lang, "备份校验失败: {error}", &[("error", &e.to_string())]))?;
        if check != "ok" {
            return Err(crate::i18n::trf(&lang, "备份文件已损坏: {check}", &[("check", &check)]));
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
        .map_err(|e| crate::i18n::trf(&lang, "备份当前库失败（已中止恢复）: {error}", &[("error", &e.to_string())]))?;

    // 用 SQLite backup API 写回：源=备份，目标=当前库
    {
        let src = rusqlite::Connection::open_with_flags(
            &backup_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .map_err(|e| crate::i18n::trf(&lang, "打开备份失败: {error}", &[("error", &e.to_string())]))?;
        let mut dst = rusqlite::Connection::open(current)
            .map_err(|e| crate::i18n::trf(&lang, "打开当前库失败: {error}", &[("error", &e.to_string())]))?;
        let backup = rusqlite::backup::Backup::new(&src, &mut dst)
            .map_err(|e| crate::i18n::trf(&lang, "初始化恢复失败: {error}", &[("error", &e.to_string())]))?;
        backup
            .run_to_completion(100, std::time::Duration::from_millis(250), None)
            .map_err(|e| crate::i18n::trf(&lang, "恢复写入失败: {error}", &[("error", &e.to_string())]))?;
    }

    // 旧备份可能缺少新版本的表/列。写回后立刻跑一次迁移，避免用户不重启就
    // 用旧结构触发 SQL 报错；迁移前会再留一份快照，且迁移本身是幂等的。
    crate::db::init_db(current).map_err(|e| crate::i18n::trf(&lang, "恢复成功，但自动升级结构失败: {error}", &[("error", &e.to_string())]))?;

    Ok(crate::i18n::trf(
        &lang,
        "已从 [{name}] 恢复并自动升级到当前结构。当前库已先备份为 {file}。",
        &[
            ("name", &name),
            ("file", &safety.file_name().unwrap_or_default().to_string_lossy().to_string()),
        ],
    ))
}
