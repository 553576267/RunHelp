use std::fs;
use std::thread;

use tauri::{AppHandle, Manager, State};

use crate::models::{LogEvent, RunEndEvent};
use crate::process::{execute, kill_tree};
use crate::state::AppState;

/// 兜底脚本：万一数据库里那条内置脚本被用户删掉（或旧库尚未 seed），
/// 用编译进二进制的副本执行，保证「立即备份」永远可用。
const BUILTIN_BACKUP_FALLBACK: &str = include_str!("../../../scripts/backup_db.py");

/// 执行内置数据库备份脚本。
///
/// 脚本源码存在 scripts 表里（name='内置数据库备份脚本'），运行时从 DB 读出
/// 写到临时文件再用配置好的 python 执行。好处：
///   1. 脚本随数据库一起备份/迁移，不依赖软件安装目录下的外部 .py 文件；
///   2. 全程用绝对路径，不受命令工作目录影响；
///   3. 用户可在「脚本管理」里查看/修改脚本源码。
///
/// 当前库路径与备份目录都通过参数显式传给脚本，避免脚本自己解析指针文件
/// 时与软件实际使用的库出现偏差。
#[tauri::command]
pub fn run_backup(app: AppHandle, state: State<'_, AppState>) -> Result<String, String> {
    let conn = state.conn()?;

    // 从数据库读内置脚本源码；读不到则用编译进二进制的副本兜底
    let script_source: String = conn
        .query_row(
            "SELECT content FROM scripts WHERE name='内置数据库备份脚本'",
            [],
            |r| r.get(0),
        )
        .unwrap_or_else(|_| BUILTIN_BACKUP_FALLBACK.to_string());

    // python 解释器
    let python_path: String = conn
        .query_row(
            "SELECT value FROM settings WHERE key='python_path'",
            [],
            |r| r.get(0),
        )
        .unwrap_or_else(|_| "python".into());

    // 备份目录：优先 settings.backup_dir，否则默认坚果云目录（与 settings.rs 一致）
    let backup_dir: String = conn
        .query_row(
            "SELECT value FROM settings WHERE key='backup_dir'",
            [],
            |r| r.get::<_, String>(0),
        )
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| {
            let home = std::env::var("USERPROFILE")
                .or_else(|_| std::env::var("HOME"))
                .unwrap_or_else(|_| "C:\\".to_string());
            std::path::PathBuf::from(home)
                .join("Nutstore")
                .join("RunHelpBackups")
                .to_string_lossy()
                .to_string()
        });

    // 临时脚本落到 app_data_dir 下的固定工作目录，与 cwd 无关
    let work_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("无法获取数据目录: {}", e))?
        .join("backup_work");
    fs::create_dir_all(&work_dir).map_err(|e| format!("创建工作目录失败: {}", e))?;
    let script_path = work_dir.join("backup_db.py");
    fs::write(&script_path, &script_source)
        .map_err(|e| format!("写入备份脚本失败: {}", e))?;

    let run_id = state.runtime.next_run_id();
    let flag = state.runtime.new_pause_flag();
    state.runtime.paused.lock().unwrap().insert(run_id.clone(), flag);
    state.runtime.running_meta.lock().unwrap().insert(
        run_id.clone(),
        crate::models::RunningMeta {
            target: "数据库备份".into(),
            started_at: state.local_now(),
        },
    );

    // 写一条运行记录
    let hist_conn = state.conn().ok();
    let history_id = if let Some(ref c) = hist_conn {
        let _ = c.execute(
            "INSERT INTO run_history(project_id, target, target_id) VALUES (0, 'backup', NULL)",
            [],
        );
        c.last_insert_rowid()
    } else {
        -1
    };

    let state_owned = state.inner().clone();
    let rid = run_id.clone();
    let app_move = app.clone();
    let py = python_path.clone();
    let script_str = script_path.to_string_lossy().to_string();
    let db_str = state.db_path.to_string_lossy().to_string();

    thread::spawn(move || {
        let _ = tauri::Emitter::emit(&app_move, "run://log", LogEvent {
            run_id: rid.clone(),
            stream: "info".into(),
            line: "开始数据库备份...".into(),
        });

        let mut proc_cmd = std::process::Command::new(&py);
        proc_cmd
            .arg(&script_str)
            .arg("--db").arg(&db_str)
            .arg("--dir").arg(&backup_dir);

        let result = execute(app_move.clone(), &state_owned, &rid, &mut proc_cmd, 0);
        let status = match result {
            Ok((code, stderr)) => {
                if code != 0 && !stderr.trim().is_empty() {
                    let _ = tauri::Emitter::emit(&app_move, "run://log", LogEvent {
                        run_id: rid.clone(),
                        stream: "stderr".into(),
                        line: stderr,
                    });
                }
                code
            }
            Err(e) => {
                let _ = tauri::Emitter::emit(&app_move, "run://log", LogEvent {
                    run_id: rid.clone(),
                    stream: "stderr".into(),
                    line: e,
                });
                -1
            }
        };
        let status_text = if status == 0 { "success" } else { "failed" };
        if let Some(ref c) = hist_conn {
            let _ = c.execute(
                "UPDATE run_history SET ended_at=datetime('now','localtime'), status=?1, exit_code=?2 WHERE id=?3",
                rusqlite::params![status_text, status, history_id],
            );
        }
        state_owned.runtime.paused.lock().unwrap().remove(&rid);
        state_owned.runtime.children.lock().unwrap().remove(&rid);
        state_owned.runtime.running_meta.lock().unwrap().remove(&rid);
        let _ = tauri::Emitter::emit(&app_move, "run://end", RunEndEvent {
            run_id: rid.clone(),
            status: status_text.into(),
            exit_code: status,
            stderr: String::new(),
        });
    });

    Ok(run_id)
}

/// 终止备份任务（复用通用 stop_run，这里仅保留导出以便将来扩展）。
#[allow(dead_code)]
pub fn _stop_backup(run_id: &str, state: &AppState) {
    let pid = state.runtime.children.lock().unwrap().get(run_id).copied();
    if let Some(p) = pid {
        kill_tree(p);
    }
}
