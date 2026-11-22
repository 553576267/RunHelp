use std::path::Path;

use tauri::State;

use crate::models::Project;
use crate::state::AppState;

/// 把项目根目录与用户填写的相对工作目录拼成完整路径。
/// 一律走 PathBuf，避免前端手写反斜杠/盘符判断；相对值为绝对路径时按 Rust 语义直接替换基准。
#[tauri::command]
pub fn resolve_project_path(
    base: String,
    relative: String,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let lang = crate::i18n::lang(&state);
    let root = base.trim();
    if root.is_empty() {
        return Err(crate::i18n::tr(&lang, "项目路径为空"));
    }
    let joined = if relative.trim().is_empty() {
        Path::new(root).to_path_buf()
    } else {
        Path::new(root).join(relative.trim())
    };
    Ok(joined.to_string_lossy().to_string())
}

/// 在项目地址上点击时，用系统文件管理器打开对应本地目录。
/// 走后端命令而非 shell:allow-open（后者当前只白名单了仓库 URL）；
/// 具体平台的文件管理器调用见 platform 层。
#[tauri::command]
pub fn open_folder(path: String, state: State<'_, AppState>) -> Result<(), String> {
    let lang = crate::i18n::lang(&state);
    if path.trim().is_empty() {
        return Err(crate::i18n::tr(&lang, "项目路径为空"));
    }
    if !Path::new(&path).is_dir() {
        return Err(crate::i18n::trf(&lang, "目录不存在: {path}", &[("path", &path)]));
    }
    // 用系统文件管理器打开；spawn 后立即返回，不等待窗口关闭
    crate::platform::open_folder(&path)
        .map_err(|e| crate::i18n::trf(&lang, "打开目录失败: {error}", &[("error", &e.to_string())]))?;
    Ok(())
}

/// 用系统默认浏览器打开外部链接。只接受 http/https，避免被当成任意协议入口。
#[tauri::command]
pub fn open_external_url(url: String, state: State<'_, AppState>) -> Result<(), String> {
    let lang = crate::i18n::lang(&state);
    let lower = url.trim().to_ascii_lowercase();
    if !(lower.starts_with("https://") || lower.starts_with("http://")) {
        return Err(crate::i18n::tr(&lang, "只支持 http/https 链接"));
    }
    crate::platform::open_url(url.trim())
        .map_err(|e| crate::i18n::trf(&lang, "打开链接失败: {error}", &[("error", &e.to_string())]))
}

fn query_projects(state: &AppState, deleted: bool) -> Result<Vec<Project>, String> {
    let conn = state.conn()?;
    let where_clause = if deleted {
        "WHERE deleted_at IS NOT NULL"
    } else {
        "WHERE deleted_at IS NULL"
    };
    let sql = format!(
        "SELECT id, name, path, type, description, created_at, updated_at FROM projects {} ORDER BY id DESC",
        where_clause
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| {
            Ok(Project {
                id: row.get(0)?,
                name: row.get(1)?,
                path: row.get(2)?,
                kind: row.get(3)?,
                description: row.get(4)?,
                created_at: row.get(5)?,
                updated_at: row.get(6)?,
            })
        })
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

#[tauri::command]
pub fn list_projects(state: State<'_, AppState>) -> Result<Vec<Project>, String> {
    query_projects(&state, false)
}

#[tauri::command]
pub fn list_deleted_projects(state: State<'_, AppState>) -> Result<Vec<Project>, String> {
    query_projects(&state, true)
}

#[tauri::command]
pub fn create_project(
    name: String,
    path: String,
    kind: String,
    description: String,
    state: State<'_, AppState>,
) -> Result<i64, String> {
    let conn = state.conn()?;
    conn.execute(
        "INSERT INTO projects(name, path, type, description) VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![name, path, kind, description],
    )
    .map_err(|e| e.to_string())?;
    Ok(conn.last_insert_rowid())
}

#[tauri::command]
pub fn update_project(
    id: i64,
    name: String,
    path: String,
    kind: String,
    description: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let conn = state.conn()?;
    conn.execute(
        "UPDATE projects SET name=?1, path=?2, type=?3, description=?4, updated_at=datetime('now','localtime') WHERE id=?5",
        rusqlite::params![name, path, kind, description, id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// 软删除：只打 deleted_at 标记，项目与命令配置都保留，可从回收站还原。
#[tauri::command]
pub fn delete_project(id: i64, state: State<'_, AppState>) -> Result<(), String> {
    let conn = state.conn()?;
    conn.execute(
        "UPDATE projects SET deleted_at=datetime('now','localtime') WHERE id=?1",
        [id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn restore_project(id: i64, state: State<'_, AppState>) -> Result<(), String> {
    let conn = state.conn()?;
    conn.execute("UPDATE projects SET deleted_at=NULL WHERE id=?1", [id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// 彻底删除单个项目（连同其命令/流水线/预设，靠外键 CASCADE）。
#[tauri::command]
pub fn permanent_delete_project(id: i64, state: State<'_, AppState>) -> Result<(), String> {
    let conn = state.conn()?;
    conn.execute("DELETE FROM projects WHERE id=?1", [id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// 清空回收站：彻底删除所有已软删除的项目。
#[tauri::command]
pub fn empty_recycle_bin(state: State<'_, AppState>) -> Result<usize, String> {
    let conn = state.conn()?;
    let n = conn
        .execute("DELETE FROM projects WHERE deleted_at IS NOT NULL", [])
        .map_err(|e| e.to_string())?;
    Ok(n)
}
