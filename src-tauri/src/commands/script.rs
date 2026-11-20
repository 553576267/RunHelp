use tauri::State;

use crate::models::Script;
use crate::state::AppState;

#[tauri::command]
pub fn list_scripts(state: State<'_, AppState>) -> Result<Vec<Script>, String> {
    let conn = state.conn()?;
    let mut stmt = conn
        .prepare("SELECT id, name, language, source_type, content, file_path, version, updated_at FROM scripts ORDER BY id")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok(Script {
                id: r.get(0)?,
                name: r.get(1)?,
                language: r.get(2)?,
                source_type: r.get(3)?,
                content: r.get(4)?,
                file_path: r.get(5)?,
                version: r.get(6)?,
                updated_at: r.get(7)?,
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
pub fn get_script(id: i64, state: State<'_, AppState>) -> Result<Script, String> {
    let conn = state.conn()?;
    conn.query_row(
        "SELECT id, name, language, source_type, content, file_path, version, updated_at FROM scripts WHERE id=?1",
        [id],
        |r| {
            Ok(Script {
                id: r.get(0)?,
                name: r.get(1)?,
                language: r.get(2)?,
                source_type: r.get(3)?,
                content: r.get(4)?,
                file_path: r.get(5)?,
                version: r.get(6)?,
                updated_at: r.get(7)?,
            })
        },
    )
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn save_script(
    id: Option<i64>,
    name: String,
    language: String,
    source_type: String,
    content: String,
    file_path: String,
    state: State<'_, AppState>,
) -> Result<i64, String> {
    let conn = state.conn()?;
    match id {
        Some(id) => {
            conn.execute(
                "UPDATE scripts SET name=?1, language=?2, source_type=?3, content=?4, file_path=?5, version=version+1, updated_at=datetime('now','localtime') WHERE id=?6",
                rusqlite::params![name, language, source_type, content, file_path, id],
            )
            .map_err(|e| e.to_string())?;
            Ok(id)
        }
        None => {
            conn.execute(
                "INSERT INTO scripts(name, language, source_type, content, file_path, version) VALUES (?1,?2,?3,?4,?5,1)",
                rusqlite::params![name, language, source_type, content, file_path],
            )
            .map_err(|e| e.to_string())?;
            Ok(conn.last_insert_rowid())
        }
    }
}

#[tauri::command]
pub fn delete_script(id: i64, state: State<'_, AppState>) -> Result<(), String> {
    let conn = state.conn()?;
    conn.execute("DELETE FROM scripts WHERE id=?1", [id])
        .map_err(|e| e.to_string())?;
    Ok(())
}
