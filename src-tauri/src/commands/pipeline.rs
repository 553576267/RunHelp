use std::thread;

use tauri::{AppHandle, State};

use crate::models::{Pipeline, PipelineStepDetail};
use crate::process::{run_shell, wait_if_paused};
use crate::state::AppState;

#[tauri::command]
pub fn list_pipelines(project_id: i64, state: State<'_, AppState>) -> Result<Vec<Pipeline>, String> {
    let conn = state.conn()?;
    let mut stmt = conn
        .prepare("SELECT id, project_id, name, description FROM pipelines WHERE project_id=?1 ORDER BY id")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([project_id], |r| {
            Ok(Pipeline {
                id: r.get(0)?,
                project_id: r.get(1)?,
                name: r.get(2)?,
                description: r.get(3)?,
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
pub fn save_pipeline(
    id: Option<i64>,
    project_id: i64,
    name: String,
    description: String,
    state: State<'_, AppState>,
) -> Result<i64, String> {
    let conn = state.conn()?;
    match id {
        Some(id) => {
            conn.execute(
                "UPDATE pipelines SET name=?1, description=?2 WHERE id=?3",
                rusqlite::params![name, description, id],
            )
            .map_err(|e| e.to_string())?;
            Ok(id)
        }
        None => {
            conn.execute(
                "INSERT INTO pipelines(project_id, name, description) VALUES (?1,?2,?3)",
                rusqlite::params![project_id, name, description],
            )
            .map_err(|e| e.to_string())?;
            Ok(conn.last_insert_rowid())
        }
    }
}

#[tauri::command]
pub fn delete_pipeline(id: i64, state: State<'_, AppState>) -> Result<(), String> {
    let conn = state.conn()?;
    conn.execute("DELETE FROM pipelines WHERE id=?1", [id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn list_pipeline_steps(pipeline_id: i64, state: State<'_, AppState>) -> Result<Vec<PipelineStepDetail>, String> {
    let conn = state.conn()?;
    let mut stmt = conn
        .prepare(
            "SELECT s.id, s.command_id, COALESCE(c.name, ''), s.step_type, s.delay_ms, s.on_error, s.sort_order \
             FROM pipeline_steps s LEFT JOIN commands c ON c.id = s.command_id \
             WHERE s.pipeline_id=?1 ORDER BY s.sort_order, s.id",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([pipeline_id], |r| {
            Ok(PipelineStepDetail {
                id: r.get(0)?,
                command_id: r.get(1)?,
                command_name: r.get(2)?,
                step_type: r.get(3)?,
                delay_ms: r.get(4)?,
                on_error: r.get(5)?,
                sort_order: r.get(6)?,
            })
        })
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

#[derive(Debug, serde::Deserialize)]
pub struct StepInput {
    pub command_id: Option<i64>,
    pub step_type: String,
    pub delay_ms: i64,
    pub on_error: String,
}

#[tauri::command]
pub fn save_pipeline_steps(
    pipeline_id: i64,
    steps: Vec<StepInput>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let mut conn = state.conn()?;
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    tx.execute("DELETE FROM pipeline_steps WHERE pipeline_id=?1", [pipeline_id])
        .map_err(|e| e.to_string())?;
    for (idx, s) in steps.iter().enumerate() {
        tx.execute(
            "INSERT INTO pipeline_steps(pipeline_id, command_id, step_type, delay_ms, on_error, sort_order) VALUES (?1,?2,?3,?4,?5,?6)",
            rusqlite::params![pipeline_id, s.command_id, s.step_type, s.delay_ms, s.on_error, idx as i64],
        )
        .map_err(|e| e.to_string())?;
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn run_pipeline(pipeline_id: i64, app: AppHandle, state: State<'_, AppState>) -> Result<String, String> {
    let conn = state.conn()?;
    let project_id: i64 = conn
        .query_row("SELECT project_id FROM pipelines WHERE id=?1", [pipeline_id], |r| r.get(0))
        .map_err(|e| format!("流水线不存在: {}", e))?;
    let project_path: String = conn
        .query_row("SELECT path FROM projects WHERE id=?1", [project_id], |r| r.get(0))
        .unwrap_or_default();

    let mut stmt = conn
        .prepare(
            "SELECT s.command_id, s.step_type, s.delay_ms, s.on_error, c.cmd, c.cwd, c.shell, c.timeout_ms \
             FROM pipeline_steps s LEFT JOIN commands c ON c.id = s.command_id \
             WHERE s.pipeline_id=?1 ORDER BY s.sort_order, s.id",
        )
        .map_err(|e| e.to_string())?;
    let rows: Vec<(Option<i64>, String, i64, String, Option<String>, Option<String>, Option<String>, Option<i64>)> = stmt
        .query_map([pipeline_id], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
                r.get(6)?,
                r.get(7)?,
            ))
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();

    let run_id = state.runtime.next_run_id();
    let flag = state.runtime.new_pause_flag();
    state.runtime.paused.lock().unwrap().insert(run_id.clone(), flag);
    let pipeline_name: String = conn
        .query_row("SELECT name FROM pipelines WHERE id=?1", [pipeline_id], |r| r.get(0))
        .unwrap_or_else(|_| format!("流水线 #{}", pipeline_id));
    state.runtime.running_meta.lock().unwrap().insert(
        run_id.clone(),
        crate::models::RunningMeta {
            target: format!("流水线: {}", pipeline_name),
            started_at: state.local_now(),
        },
    );

    let state_owned = state.inner().clone();
    let rid = run_id.clone();
    thread::spawn(move || {
        let hist_conn = state_owned.conn();
        let history_id = if let Ok(ref c) = hist_conn {
            let _ = c.execute(
                "INSERT INTO run_history(project_id, target, target_id) VALUES (?1,?2,?3)",
                rusqlite::params![project_id, "pipeline", pipeline_id],
            );
            c.last_insert_rowid()
        } else {
            -1
        };

        let mut aborted = false;
        for (idx, (cmd_id, step_type, delay_ms, on_error, m_cmd, m_cwd, m_shell, m_timeout)) in rows.iter().enumerate() {
            wait_if_paused(&state_owned, &rid);
            if aborted || state_owned.runtime.is_stopped(&rid) {
                aborted = true;
                break;
            }
            if step_type == "delay" {
                thread::sleep(std::time::Duration::from_millis((*delay_ms).max(0) as u64));
                continue;
            }
            if let (Some(cmd), Some(shell)) = (m_cmd, m_shell) {
                let rel_cwd = m_cwd.clone().unwrap_or_default();
                let cwd = if rel_cwd.is_empty() {
                    project_path.clone()
                } else if std::path::Path::new(&rel_cwd).is_absolute() {
                    rel_cwd.clone()
                } else {
                    std::path::Path::new(&project_path).join(&rel_cwd).to_string_lossy().to_string()
                };
                let step_rid = format!("{}-s{}", rid, idx);
                let _ = tauri::Emitter::emit(&app, "run://log", crate::models::LogEvent {
                    run_id: rid.clone(),
                    stream: "info".into(),
                    line: format!("步骤 {}: 执行中", idx + 1),
                });
                let result = run_shell(
                    app.clone(),
                    &state_owned,
                    &step_rid,
                    cmd,
                    &cwd,
                    shell,
                    m_timeout.unwrap_or(0),
                );
                let failed = match result {
                    Ok((code, _)) => code != 0,
                    Err(_) => true,
                };
                let _ = tauri::Emitter::emit(&app, "run://log", crate::models::LogEvent {
                    run_id: rid.clone(),
                    stream: "info".into(),
                    line: format!("步骤 {} 完成: {}", idx + 1, if failed { "失败" } else { "成功" }),
                });
                if failed && on_error == "stop" {
                    aborted = true;
                }
                let _ = cmd_id;
            }
        }
        if let Ok(ref c) = hist_conn {
            let _ = c.execute(
                "UPDATE run_history SET ended_at=datetime('now','localtime'), status=?1 WHERE id=?2",
                rusqlite::params![if aborted { "failed" } else { "success" }, history_id],
            );
        }
        state_owned.runtime.paused.lock().unwrap().remove(&rid);
        state_owned.runtime.running_meta.lock().unwrap().remove(&rid);
    });

    Ok(run_id)
}
