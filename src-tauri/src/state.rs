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
    /// 数据同步运行态（设备列表、配对码、待确认请求等）
    pub sync: Arc<crate::sync::SyncRuntime>,
    /// app.handle() 的持有者：供后台 HTTP 线程发射事件用
    pub app_handle: Arc<Mutex<Option<tauri::AppHandle>>>,
}

impl AppState {
    pub fn new(db_path: PathBuf, sync: Arc<crate::sync::SyncRuntime>) -> Self {
        Self {
            db_path,
            runtime: Arc::new(RuntimeState::new()),
            sync,
            app_handle: Arc::new(Mutex::new(None)),
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

    // 以下静态访问器供后台线程（HTTP 服务）使用：它们拿不到 tauri::State，
    // 因此用一个 OnceLock 保存 AppState 的克隆。只在 setup 里赋值一次。

    fn global() -> Option<&'static AppState> {
        GLOBAL_STATE.get()
    }

    pub fn set_global(state: AppState) {
        let _ = GLOBAL_STATE.set(state);
    }

    pub fn open_db_for_sync() -> Option<Connection> {
        Self::global().and_then(|s| s.conn().ok())
    }

    pub fn app_handle_for_sync() -> Option<tauri::AppHandle> {
        Self::global()
            .and_then(|s| s.app_handle.lock().ok().and_then(|g| g.clone()))
    }

    pub fn sync_holder_for_sync() -> Option<Arc<crate::sync::SyncRuntime>> {
        Self::global().map(|s| s.sync.clone())
    }
}

static GLOBAL_STATE: std::sync::OnceLock<AppState> = std::sync::OnceLock::new();
