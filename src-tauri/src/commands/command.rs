use std::thread;

use tauri::{AppHandle, State};

use crate::models::Command;
use crate::process::{kill_tree, run_shell, wait_if_paused};
use crate::state::AppState;

#[tauri::command]
pub fn list_commands(project_id: i64, state: State<'_, AppState>) -> Result<Vec<Command>, String> {
    let conn = state.conn()?;
    let mut stmt = conn
        .prepare("SELECT id, project_id, name, cmd, cwd, shell, type, timeout_ms, sort_order FROM commands WHERE project_id=?1 ORDER BY sort_order, id")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([project_id], |row| {
            Ok(Command {
                id: row.get(0)?,
                project_id: row.get(1)?,
                name: row.get(2)?,
                cmd: row.get(3)?,
                cwd: row.get(4)?,
                shell: row.get(5)?,
                kind: row.get(6)?,
                timeout_ms: row.get(7)?,
                sort_order: row.get(8)?,
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
#[allow(clippy::too_many_arguments)]
pub fn save_command(
    id: Option<i64>,
    project_id: i64,
    name: String,
    cmd: String,
    cwd: String,
    shell: String,
    kind: String,
    timeout_ms: i64,
    sort_order: i64,
    state: State<'_, AppState>,
) -> Result<i64, String> {
    let conn = state.conn()?;
    match id {
        Some(id) => {
            conn.execute(
                "UPDATE commands SET name=?1, cmd=?2, cwd=?3, shell=?4, type=?5, timeout_ms=?6, sort_order=?7 WHERE id=?8",
                rusqlite::params![name, cmd, cwd, shell, kind, timeout_ms, sort_order, id],
            )
            .map_err(|e| e.to_string())?;
            Ok(id)
        }
        None => {
            conn.execute(
                "INSERT INTO commands(project_id, name, cmd, cwd, shell, type, timeout_ms, sort_order) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
                rusqlite::params![project_id, name, cmd, cwd, shell, kind, timeout_ms, sort_order],
            )
            .map_err(|e| e.to_string())?;
            Ok(conn.last_insert_rowid())
        }
    }
}

#[tauri::command]
pub fn delete_command(id: i64, state: State<'_, AppState>) -> Result<(), String> {
    let conn = state.conn()?;
    conn.execute("DELETE FROM commands WHERE id=?1", [id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn record_history(state: &AppState, project_id: i64, target: &str, target_id: i64) -> i64 {
    let conn = match state.conn() {
        Ok(c) => c,
        Err(_) => return -1,
    };
    let _ = conn.execute(
        "INSERT INTO run_history(project_id, target, target_id) VALUES (?1,?2,?3)",
        rusqlite::params![project_id, target, target_id],
    );
    conn.last_insert_rowid()
}

fn finish_history(state: &AppState, id: i64, status: &str, code: i32) {
    if id <= 0 {
        return;
    }
    if let Ok(conn) = state.conn() {
        let _ = conn.execute(
            "UPDATE run_history SET ended_at=datetime('now','localtime'), exit_code=?1, status=?2 WHERE id=?3",
            rusqlite::params![code, status, id],
        );
    }
}

#[tauri::command]
pub fn run_command(command_id: i64, app: AppHandle, state: State<'_, AppState>) -> Result<String, String> {
    let conn = state.conn()?;
    let lang = crate::i18n::lang(state.inner());
    let row: (i64, String, String, String, String, i64) = conn
        .query_row(
            "SELECT c.project_id, c.cmd, c.cwd, c.shell, c.name, c.timeout_ms FROM commands c WHERE c.id=?1",
            [command_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
        )
        .map_err(|e| crate::i18n::trf(&lang, "命令不存在: {error}", &[("error", &e.to_string())]))?;
    let (project_id, cmd, rel_cwd, shell, name, timeout) = row;

    let project_path: String = conn
        .query_row("SELECT path FROM projects WHERE id=?1", [project_id], |r| r.get(0))
        .unwrap_or_default();

    let cwd = if rel_cwd.is_empty() {
        project_path
    } else if std::path::Path::new(&rel_cwd).is_absolute() {
        rel_cwd
    } else {
        std::path::Path::new(&project_path).join(&rel_cwd).to_string_lossy().to_string()
    };

    let run_id = state.runtime.next_run_id();
    let flag = state.runtime.new_pause_flag();
    state.runtime.paused.lock().unwrap().insert(run_id.clone(), flag);

    let rid = run_id.clone();
    let app_move = app.clone();
    let state_arc = state.inner().clone();
    state.runtime.running_meta.lock().unwrap().insert(
        run_id.clone(),
        crate::models::RunningMeta {
            target: name.clone(),
            started_at: state.local_now(),
            command_id: Some(command_id),
            pipeline_id: None,
        },
    );
    thread::spawn(move || {
        let history_id = record_history(&state_arc, project_id, &format!("command:{}", name), command_id);
        state_arc.runtime.history_ids.lock().unwrap().insert(rid.clone(), history_id);
        wait_if_paused(&state_arc, &rid);
        let result = run_shell(app_move, &state_arc, &rid, &cmd, &cwd, &shell, timeout, &name);
        match result {
            Ok((code, status)) => finish_history(&state_arc, history_id, &status, code),
            Err(_) => finish_history(&state_arc, history_id, "failed", -1),
        }
        state_arc.runtime.paused.lock().unwrap().remove(&rid);
    });

    Ok(run_id)
}

#[tauri::command]
pub fn run_adhoc(
    cmd: String,
    cwd: String,
    shell: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let lang = crate::i18n::lang(state.inner());
    let run_id = state.runtime.next_run_id();
    let flag = state.runtime.new_pause_flag();
    state.runtime.paused.lock().unwrap().insert(run_id.clone(), flag);
    state.runtime.running_meta.lock().unwrap().insert(
        run_id.clone(),
        crate::models::RunningMeta {
            target: crate::i18n::trf(&lang, "临时: {cmd}", &[("cmd", &cmd)]),
            started_at: state.local_now(),
            command_id: None,
            pipeline_id: None,
        },
    );
    let rid = run_id.clone();
    let state_owned = state.inner().clone();
    thread::spawn(move || {
        wait_if_paused(&state_owned, &rid);
        let label = crate::i18n::tr(&lang, "临时命令");
        let _ = run_shell(app, &state_owned, &rid, &cmd, &cwd, &shell, 0, &label);
        state_owned.runtime.paused.lock().unwrap().remove(&rid);
    });
    Ok(run_id)
}

#[tauri::command]
pub fn stop_run(run_id: String, state: State<'_, AppState>) -> Result<(), String> {
    // 标记为"用户主动终止"：execute() 结束后据此判为 stopped 而非 failed
    state.runtime.stopped.lock().unwrap().insert(run_id.clone());
    // 收集目标 run_id 及其流水线 step（{run_id}-s{N}）对应的所有 pid，全部终止。
    let pids: Vec<u32> = {
        let children = state.runtime.children.lock().unwrap();
        let prefix = format!("{}-s", run_id);
        children
            .iter()
            .filter(|(k, _)| k.as_str() == run_id || k.starts_with(&prefix))
            .map(|(_, v)| *v)
            .collect()
    };
    for p in pids {
        kill_tree(p);
    }
    // 立刻收尾历史记录（execute 线程可能稍后才结束），避免界面停留"运行中"
    let history_id = state
        .runtime
        .history_ids
        .lock()
        .unwrap()
        .get(&run_id)
        .copied()
        .unwrap_or(-1);
    finish_history(&state, history_id, "stopped", -1073741510);
    Ok(())
}

#[tauri::command]
pub fn pause_run(run_id: String, state: State<'_, AppState>) -> Result<(), String> {
    use std::sync::atomic::Ordering;
    if let Some(flag) = state.runtime.paused.lock().unwrap().get(&run_id) {
        flag.store(true, Ordering::SeqCst);
    }
    Ok(())
}

#[tauri::command]
pub fn resume_run(run_id: String, state: State<'_, AppState>) -> Result<(), String> {
    use std::sync::atomic::Ordering;
    if let Some(flag) = state.runtime.paused.lock().unwrap().get(&run_id) {
        flag.store(false, Ordering::SeqCst);
    }
    Ok(())
}

/// 列出当前所有运行中的任务（供界面提供"终止"入口）。
/// 以 running_meta 为主（覆盖命令/临时/流水线/压缩），pid 从 children 精确匹配，
/// 流水线的 step 子进程 key 为 `{run_id}-s{N}`，用前缀回退取第一个存活 pid。
#[tauri::command]
pub fn list_running(state: State<'_, AppState>) -> Result<Vec<crate::models::RunningInfo>, String> {
    let meta = state.runtime.running_meta.lock().unwrap().clone();
    let children = state.runtime.children.lock().unwrap().clone();
    let mut out = Vec::new();
    for (run_id, m) in meta {
        let pid = children
            .get(&run_id)
            .copied()
            .or_else(|| {
                let prefix = format!("{}-s", run_id);
                children
                    .iter()
                    .filter(|(k, _)| k.starts_with(&prefix))
                    .map(|(_, v)| *v)
                    .next()
            })
            .unwrap_or(0);
        // 暂停标志以父 run_id 存入（流水线 step 为 {run_id}-s{N}），直接按 meta 的 key 查询。
        let paused = state
            .runtime
            .paused
            .lock()
            .unwrap()
            .get(&run_id)
            .map(|f| f.load(std::sync::atomic::Ordering::SeqCst))
            .unwrap_or(false);
        out.push(crate::models::RunningInfo {
            run_id,
            target: m.target,
            started_at: m.started_at,
            pid,
            paused,
            command_id: m.command_id,
            pipeline_id: m.pipeline_id,
        });
    }
    Ok(out)
}

/// 清空全部运行记录。
#[tauri::command]
pub fn clear_history(state: State<'_, AppState>) -> Result<(), String> {
    let conn = state.conn()?;
    conn.execute("DELETE FROM run_history", [])
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn list_history(limit: i64, state: State<'_, AppState>) -> Result<Vec<crate::models::RunHistory>, String> {
    let conn = state.conn()?;
    let mut stmt = conn
        .prepare("SELECT id, project_id, target, target_id, started_at, ended_at, exit_code, status, log_path FROM run_history ORDER BY id DESC LIMIT ?1")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([limit], |r| {
            Ok(crate::models::RunHistory {
                id: r.get(0)?,
                project_id: r.get(1)?,
                target: r.get(2)?,
                target_id: r.get(3)?,
                started_at: r.get(4)?,
                ended_at: r.get(5)?,
                exit_code: r.get(6)?,
                status: r.get(7)?,
                log_path: r.get(8)?,
            })
        })
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}
