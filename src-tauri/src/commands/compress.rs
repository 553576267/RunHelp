use std::fs;
use std::thread;

use tauri::{AppHandle, State};

use crate::models::{CompressionProfile, LogEvent};
use crate::process::{execute, kill_tree, wait_if_paused};
use crate::state::AppState;

#[tauri::command]
pub fn list_profiles(project_id: i64, state: State<'_, AppState>) -> Result<Vec<CompressionProfile>, String> {
    let conn = state.conn()?;
    let mut stmt = conn
        .prepare("SELECT id, project_id, name, params_json, script_id, created_at FROM compression_profiles WHERE project_id=?1 ORDER BY id")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([project_id], |r| {
            Ok(CompressionProfile {
                id: r.get(0)?,
                project_id: r.get(1)?,
                name: r.get(2)?,
                params_json: r.get(3)?,
                script_id: r.get(4)?,
                created_at: r.get(5)?,
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
pub fn save_profile(
    id: Option<i64>,
    project_id: i64,
    name: String,
    params_json: String,
    script_id: Option<i64>,
    state: State<'_, AppState>,
) -> Result<i64, String> {
    let conn = state.conn()?;
    match id {
        Some(id) => {
            conn.execute(
                "UPDATE compression_profiles SET name=?1, params_json=?2, script_id=?3 WHERE id=?4",
                rusqlite::params![name, params_json, script_id, id],
            )
            .map_err(|e| e.to_string())?;
            Ok(id)
        }
        None => {
            conn.execute(
                "INSERT INTO compression_profiles(project_id, name, params_json, script_id) VALUES (?1,?2,?3,?4)",
                rusqlite::params![project_id, name, params_json, script_id],
            )
            .map_err(|e| e.to_string())?;
            Ok(conn.last_insert_rowid())
        }
    }
}

#[tauri::command]
pub fn delete_profile(id: i64, state: State<'_, AppState>) -> Result<(), String> {
    let conn = state.conn()?;
    conn.execute("DELETE FROM compression_profiles WHERE id=?1", [id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn run_compression(
    profile_id: i64,
    input_paths: Vec<String>,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let conn = state.conn()?;
    let (params_json, script_id): (String, Option<i64>) = conn
        .query_row(
            "SELECT params_json, script_id FROM compression_profiles WHERE id=?1",
            [profile_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(|e| format!("预设不存在: {}", e))?;

    let mut params: serde_json::Value =
        serde_json::from_str(&params_json).map_err(|e| format!("参数格式错误: {}", e))?;
    if let Some(obj) = params.as_object_mut() {
        obj.insert(
            "input_paths".into(),
            serde_json::Value::Array(input_paths.into_iter().map(serde_json::Value::String).collect()),
        );
    }

    let python_path: String = conn
        .query_row(
            "SELECT value FROM settings WHERE key='python_path'",
            [],
            |r| r.get(0),
        )
        .unwrap_or_else(|_| "python".into());

    let script_source: String = match script_id {
        Some(sid) => conn
            .query_row(
                "SELECT source_type, content, file_path FROM scripts WHERE id=?1",
                [sid],
                |r| {
                    let st: String = r.get(0)?;
                    if st == "file" {
                        let fp: String = r.get(2)?;
                        fs::read_to_string(&fp).map_err(|_| rusqlite::Error::InvalidQuery)
                    } else {
                        r.get(1)
                    }
                },
            )
            .map_err(|e| format!("读取脚本失败: {}", e))?,
        None => conn
            .query_row(
                "SELECT content FROM scripts WHERE name='内置图片压缩脚本'",
                [],
                |r| r.get(0),
            )
            .map_err(|e| format!("读取内置脚本失败: {}", e))?,
    };

    let work_dir = state
        .db_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .join("compress_work");
    fs::create_dir_all(&work_dir).map_err(|e| e.to_string())?;

    let script_path = work_dir.join("compress_images.py");
    let config_path = work_dir.join("config.json");
    fs::write(&script_path, &script_source).map_err(|e| format!("写入脚本失败: {}", e))?;
    fs::write(&config_path, serde_json::to_string_pretty(&params).unwrap_or_default())
        .map_err(|e| format!("写入配置失败: {}", e))?;

    let run_id = state.runtime.next_run_id();
    let flag = state.runtime.new_pause_flag();
    state.runtime.paused.lock().unwrap().insert(run_id.clone(), flag);
    let profile_name: String = conn
        .query_row("SELECT name FROM compression_profiles WHERE id=?1", [profile_id], |r| r.get(0))
        .unwrap_or_else(|_| format!("压缩预设 #{}", profile_id));
    state.runtime.running_meta.lock().unwrap().insert(
        run_id.clone(),
        crate::models::RunningMeta {
            target: format!("图片压缩: {}", profile_name),
            started_at: state.local_now(),
            command_id: None,
            pipeline_id: None,
        },
    );

    let state_owned = state.inner().clone();
    let rid = run_id.clone();
    let app_move = app.clone();
    let py = python_path.clone();
    let script_str = script_path.to_string_lossy().to_string();
    let config_str = config_path.to_string_lossy().to_string();
    let hist_conn = state.conn().ok();
    let history_id = if let Some(ref c) = hist_conn {
        let _ = c.execute(
            "INSERT INTO run_history(project_id, target, target_id) VALUES (?1,?2,?3)",
            rusqlite::params![conn.query_row("SELECT project_id FROM compression_profiles WHERE id=?1", [profile_id], |r| r.get::<_, i64>(0)).unwrap_or(0), "compress", profile_id],
        );
        c.last_insert_rowid()
    } else {
        -1
    };

    thread::spawn(move || {
        wait_if_paused(&state_owned, &rid);
        let _ = tauri::Emitter::emit(&app_move, "run://log", LogEvent {
            run_id: rid.clone(),
            stream: "info".into(),
            line: "开始压缩...".into(),
        });
        let mut proc_cmd = std::process::Command::new(&py);
        proc_cmd.arg(&script_str).arg("--config").arg(&config_str);
        let result = execute(app_move.clone(), &state_owned, &rid, &mut proc_cmd, 0, "图片压缩");
        let status = match result {
            Ok((code, _)) => code,
            Err(e) => {
                let _ = tauri::Emitter::emit(&app_move, "run://log", LogEvent {
                    run_id: rid.clone(),
                    stream: "stderr".into(),
                    line: e,
                });
                -1
            }
        };
        if let Some(ref c) = hist_conn {
            let _ = c.execute(
                "UPDATE run_history SET ended_at=datetime('now','localtime'), status=?1, exit_code=?2 WHERE id=?3",
                rusqlite::params![if status == 0 { "success" } else { "failed" }, status, history_id],
            );
        }
        state_owned.runtime.paused.lock().unwrap().remove(&rid);
    });

    Ok(run_id)
}

#[tauri::command]
pub fn stop_compression(run_id: String, state: State<'_, AppState>) -> Result<(), String> {
    let pid = state.runtime.children.lock().unwrap().get(&run_id).copied();
    if let Some(p) = pid {
        kill_tree(p);
    }
    Ok(())
}
