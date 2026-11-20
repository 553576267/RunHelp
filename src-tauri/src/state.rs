use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use rusqlite::Connection;

pub struct RuntimeState {
    pub children: Mutex<HashMap<String, u32>>,
    pub paused: Mutex<HashMap<String, Arc<AtomicBool>>>,
    /// run_id -> run_history.id，用于终止时把对应历史记录标记为 stopped
    pub history_ids: Mutex<HashMap<String, i64>>,
    /// 被用户主动终止的 run_id 集合，避免结束后又被判成 failed
    pub stopped: Mutex<std::collections::HashSet<String>>,
    /// run_id -> 展示信息（任务名/启动时间），供"运行中任务"列表使用
    pub running_meta: Mutex<HashMap<String, crate::models::RunningMeta>>,
    pub seq: AtomicI64,
}

impl RuntimeState {
    pub fn new() -> Self {
        Self {
            children: Mutex::new(HashMap::new()),
            paused: Mutex::new(HashMap::new()),
            history_ids: Mutex::new(HashMap::new()),
            stopped: Mutex::new(std::collections::HashSet::new()),
            running_meta: Mutex::new(HashMap::new()),
            seq: AtomicI64::new(0),
        }
    }

    pub fn next_run_id(&self) -> String {
        let n = self.seq.fetch_add(1, Ordering::SeqCst) + 1;
        format!("r{}", n)
    }

    pub fn new_pause_flag(&self) -> Arc<AtomicBool> {
        Arc::new(AtomicBool::new(false))
    }

    /// run_id 是否已被用户终止（含流水线 step: r1-s0 归到父 r1）。
    pub fn is_stopped(&self, run_id: &str) -> bool {
        let parent = run_id.split("-s").next().unwrap_or(run_id);
        self.stopped
            .lock()
            .map(|s| s.contains(run_id) || s.contains(parent))
            .unwrap_or(false)
    }
}

#[derive(Clone)]
pub struct AppState {
    pub db_path: PathBuf,
    pub runtime: Arc<RuntimeState>,
}

impl AppState {
    pub fn new(db_path: PathBuf) -> Self {
        Self {
            db_path,
            runtime: Arc::new(RuntimeState::new()),
        }
    }

    pub fn conn(&self) -> Result<Connection, String> {
        Connection::open(&self.db_path).map_err(|e| e.to_string())
    }

    /// 本地时间字符串（复用 SQLite，避免引入 chrono 依赖）。
    pub fn local_now(&self) -> String {
        self.conn()
            .ok()
            .and_then(|c| {
                c.query_row("SELECT datetime('now','localtime')", [], |r| {
                    r.get::<_, String>(0)
                })
                .ok()
            })
            .unwrap_or_default()
    }
}
