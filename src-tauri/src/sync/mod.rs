//! 数据同步：局域网内设备发现与端到端传输。
//!
//! 设计要点：
//! - 设备发现用 UDP 广播（不依赖 mDNS 系统服务，跨机器更稳定），端口 47800；广播携带证书指纹。
//! - 传输用内置 mTLS 服务（rustls 同步 API + 自解析 HTTP），端口 47801；两端互为收发。
//! - 每台设备一张自签证书，证书 SHA-256 指纹 = 设备身份证；私钥只在本机，无法伪造。
//! - TLS 双向握手（验对方持有证书私钥），应用层再校验「声明指纹 == 证书指纹」防冒充。
//! - 首次连接弹指纹确认框（TOFU），确认后记住指纹，之后免确认；可开启自动接收。
//! - 目录按相对路径逐文件传输，接收端原样重建。

pub mod commands;
pub mod discovery;
pub mod identity;
pub mod transfer;

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{mpsc, Arc, Mutex};

use identity::Identity;

/// UDP 设备发现端口
pub const DISCOVERY_PORT: u16 = 47800;
/// HTTP 传输服务端口
pub const TRANSFER_PORT: u16 = 47801;
/// 协议标识（防止误收其它 UDP 包）
pub const DISCOVERY_MAGIC: &str = "RUNHELP1";
/// 设备多久没收到广播就判定离线（秒）。
/// 每秒广播一次，这里给到 4 秒（容忍连丢 3 包）：上线是「收到第一包」即刻，
/// 离线只能靠超时，所以必须小；稳定性交给应用层存活探测兜底（探测成功会刷新 last_probe）。
/// 注意别等于 LIVENESS_INTERVAL_SECS，否则超时判定与探测刷新卡在同一秒边界会抖。
pub const DEVICE_TIMEOUT_SECS: i64 = 4;
/// 只靠网段扫描发现的设备（没有广播心跳）的兜底超时。
/// 这类设备现在也会被存活探测覆盖（每 2 秒刷 last_probe），正常情况下用不到这个值；
/// 它只是探测线程异常时的最后一道保险。**必须大于扫描间隔的 3 倍**：
/// 曾经取 12 秒 < 扫描间隔 15 秒，导致「每 15 秒闪一次离线」的确定性抖动。
pub const PROBE_DEVICE_TIMEOUT_SECS: i64 = 45;
/// 探测存活有效期（秒）：mTLS /hello 成功一次即认为设备还活着。
/// 取 8 秒 = 探测周期 2s × 2 + 单轮最长约 1s + 3s 余量。
/// 曾经取 5 秒：只要某一轮稍慢（多设备并行握手、Wi-Fi 省电、正在传大文件），
/// 「上一轮成功」到「下一轮成功」的间隔就会顶到 5 秒边界，设备被时间戳判离线，
/// 紧接着下一次探测又成功 → 界面上一秒离线一秒在线地来回切。
pub const PROBE_FRESH_SECS: i64 = 8;
/// 近期设备存活探测周期（秒）。2 秒一轮，单台超时 0.9 秒且全部并行，
/// 一轮总耗时通常不超过 1 秒。
pub const LIVENESS_INTERVAL_SECS: u64 = 2;
/// 连续探测失败达到该次数才判定离线，避免单次握手抖动导致闪断。
/// 取 3 次 × 2 秒 ≈ 6 秒：与 PROBE_FRESH_SECS 对齐（第 3 次失败时 last_probe 还没过期，
/// 由计数而不是时间戳下判断，两个判据不会互相打架）。
/// 探测内部不再重试（见 transfer::probe_liveness），重试即下一轮。
pub const PROBE_OFFLINE_MISSES: u8 = 3;
/// 网段扫描周期（秒）。只是广播被 AP 隔离时的兜底，别太密：
/// 每轮要打 254 个 TCP SYN + 少量 mTLS 握手，扫得太勤本身就抬高了探测超时的概率。
pub const SUBNET_SCAN_INTERVAL_SECS: u64 = 20;
/// 设备状态诊断日志条数上限（环形缓冲，供同步页「设备日志」查看）
pub const STATE_LOG_CAP: usize = 300;

/// 接收确认超时（秒）：超时未点则自动拒绝，避免 HTTP 线程永久挂起。
/// 用户反馈 90 秒偏短，这里放宽到 5 分钟；取消发送会立即解除等待。
pub const REQUEST_TIMEOUT_SECS: u64 = 300;
/// 客户端等待响应超时（秒），略大于接收确认超时，避免先于对端超时断线。
pub const CLIENT_READ_TIMEOUT_SECS: u64 = REQUEST_TIMEOUT_SECS + 30;

/// 发现到的设备（内存态，重启即失效；可信关系落库到 sync_devices）
#[derive(Clone, Debug, serde::Serialize)]
pub struct DeviceInfo {
    pub id: String,
    pub name: String,
    pub platform: String,
    pub ip: String,
    pub port: u16,
    /// 证书指纹（设备身份证）
    pub fingerprint: String,
    /// 最近一次收到广播的 Unix 秒（只由广播/心跳刷新）
    pub last_seen: i64,
    /// 最近一次 mTLS 探测成功的 Unix 秒（只由 /hello 探测刷新）。
    /// 与 last_seen 分开计时：热点下广播可能被隔离，靠探测也能维持在线判定，
    /// 又不会被「探测周期 == 广播超时」卡边界导致抖断。
    #[serde(default)]
    pub last_probe: i64,
    pub trusted: bool,
    pub auto_accept: bool,
    /// 是否收藏（常用设备）
    pub favorite: bool,
    /// 用户自定义别名
    pub alias: String,
    /// 上次发现的 IP（离线时展示）
    pub last_ip: String,
    /// 是否在线（最近广播未超时）
    pub online: bool,
    /// 是否仅靠网段探测发现（无广播心跳，需更长的离线超时）
    pub probe_only: bool,
}

impl DeviceInfo {
    /// 在线判定：广播心跳新鲜，或最近一次探测成功且没过期。
    /// 两者取「或」：热点下广播可能被 AP 隔离，但 TCP 探测仍能通，设备不该被判离线；
    /// 反过来广播还在、探测偶尔超时（手机省电导致握手慢）时也不会闪断。
    pub fn is_online(&self, now: i64) -> bool {
        let timeout = if self.probe_only {
            PROBE_DEVICE_TIMEOUT_SECS
        } else {
            DEVICE_TIMEOUT_SECS
        };
        let heartbeat = self.last_seen > 0 && now - self.last_seen < timeout;
        let probed = self.last_probe > 0 && now - self.last_probe < PROBE_FRESH_SECS;
        heartbeat || probed
    }
}

/// 待接收清单中的单项（文件/文字）
#[derive(Clone, Debug, serde::Serialize)]
pub struct PendingItem {
    pub name: String,
    pub size: u64,
}

/// 设备上下线诊断记录。
///
/// 排查「设备偶尔离线又上线来回切」时，光看界面只能知道抖了，不知道是谁判的：
/// 广播超时、探测连续失败、手动刷新 strict、BYE、还是扫描兜底过期，处理方式完全不同。
/// 每次状态翻转（以及「在线设备探测失败第 N 次」）都记一条，界面可见也可导出。
#[derive(Clone, Debug, serde::Serialize)]
pub struct StateLogEntry {
    /// Unix 秒
    pub ts: i64,
    /// info 上线 / warn 离线与失败
    pub level: String,
    /// 设备名（离线后也可能取不到，用 id 兜底）
    pub device: String,
    /// 已格式化好的一行文本，界面直接显示
    pub line: String,
}

/// 待前端确认的接收请求
pub struct PendingRequest {
    pub id: String,
    /// 一次发送操作的临时 ID：同一批文件共用一个，便于取消和前端分组展示
    pub transfer_id: String,
    pub from_id: String,
    pub from_name: String,
    /// 发送方平台（windows / linux / darwin ...）
    pub from_platform: String,
    /// 对端证书指纹（TOFU 时展示/记住）
    pub from_fp: String,
    /// text / clipboard / file / folder
    pub kind: String,
    pub name: String,
    pub size: u64,
    pub count: usize,
    /// 供接收页勾选接收/拒绝的明细；文字类只有一个项
    pub items: Vec<PendingItem>,
    /// 响应内容：all / none / 逗号分隔的接收索引
    pub respond: mpsc::Sender<String>,
    /// 发送端主动撤销时置位，服务端立即结束等待
    pub canceled: Arc<AtomicBool>,
}

/// 同步服务运行态（挂在 AppState 上，全局共享）
pub struct SyncRuntime {
    /// device_id -> 设备信息
    pub devices: Mutex<HashMap<String, DeviceInfo>>,
    /// request_id -> 待确认请求
    pub pending: Mutex<HashMap<String, PendingRequest>>,
    /// transfer_id -> 发送端取消标志（取消后停止后续文件发送）
    pub active_sends: Mutex<HashMap<String, Arc<AtomicBool>>>,
    /// 本机设备身份（自签证书 + 指纹），启动时加载/生成
    pub identity: Identity,
    /// 已信任的对端指纹集合（TOFU 后写入）
    pub trusted_fps: Arc<Mutex<HashSet<String>>>,
    pub discovery_stop: Arc<AtomicBool>,
    pub server_stop: Arc<AtomicBool>,
    pub started: AtomicBool,
    pub req_seq: AtomicI64,
    /// 主动探测并发保护：UI 打开/手动刷新/后台存活探测共用，避免重复探测。
    pub probe_inflight: AtomicBool,
    /// 近期设备连续探测失败次数（指纹 -> 次数），避免一次抖动误判离线。
    pub probe_misses: Mutex<HashMap<String, u8>>,
    /// 发现层标记「设备列表有变化（新设备/由离线转在线）」，
    /// 由存活探测线程消费后推 `sync://devices-updated`。
    /// 发现线程本身没有 AppHandle，无法直接发事件；靠这个标志把「秒级上线」也事件化，
    /// 不用等前端 5 秒一次的兜底轮询。
    pub device_dirty: AtomicBool,
    /// 设备上下线诊断日志（环形，最多 STATE_LOG_CAP 条）
    pub state_log: Mutex<std::collections::VecDeque<StateLogEntry>>,
}

impl SyncRuntime {
    pub fn new(identity: Identity, trusted_fps: HashSet<String>) -> Self {
        Self {
            devices: Mutex::new(HashMap::new()),
            pending: Mutex::new(HashMap::new()),
            active_sends: Mutex::new(HashMap::new()),
            identity,
            trusted_fps: Arc::new(Mutex::new(trusted_fps)),
            discovery_stop: Arc::new(AtomicBool::new(false)),
            server_stop: Arc::new(AtomicBool::new(false)),
            started: AtomicBool::new(false),
            req_seq: AtomicI64::new(0),
            probe_inflight: AtomicBool::new(false),
            probe_misses: Mutex::new(HashMap::new()),
            device_dirty: AtomicBool::new(false),
            state_log: Mutex::new(std::collections::VecDeque::new()),
        }
    }

    pub fn next_request_id(&self) -> String {
        format!("q{}", self.req_seq.fetch_add(1, Ordering::SeqCst) + 1)
    }
}

/// 生成设备 ID：本机名 + 随机尾巴，避免同名机器冲突
pub fn generate_device_id(name: &str) -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let tail = (millis ^ (millis >> 13) ^ (std::process::id() as u64)) % 0x10000;
    format!("{}-{:04x}", name, tail)
}

/// 当前 Unix 秒
pub fn now_secs() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 把时间戳描述成「3.2s 前」，未记录时返回「无」。
/// 诊断日志里必须带这个：只说「离线」无法区分是广播丢了还是探测失败了。
pub fn age_desc(now: i64, ts: i64) -> String {
    if ts <= 0 {
        "无".to_string()
    } else {
        format!("{:.1}s 前", (now - ts).max(0) as f64)
    }
}

/// 读取本机显示名（仅同步功能使用，用户可在同步页修改）
pub fn display_name(conn: &rusqlite::Connection) -> String {
    conn.query_row(
        "SELECT value FROM settings WHERE key='device_name'",
        [],
        |r| r.get::<_, String>(0),
    )
    .ok()
    .filter(|s| !s.trim().is_empty())
    .unwrap_or_else(|| {
        std::env::var("COMPUTERNAME")
            .or_else(|_| std::env::var("HOSTNAME"))
            .unwrap_or_else(|_| "RunHelp 设备".to_string())
    })
}

/// 读取本机稳定 ID：存 settings.device_id，重启不变（配对可信依赖它）
pub fn device_id(conn: &rusqlite::Connection) -> String {
    if let Ok(id) = conn.query_row(
        "SELECT value FROM settings WHERE key='device_id'",
        [],
        |r| r.get::<_, String>(0),
    ) {
        if !id.trim().is_empty() {
            return id;
        }
    }
    let id = generate_device_id(&display_name(conn));
    let _ = conn.execute(
        "INSERT OR REPLACE INTO settings(key, value) VALUES('device_id', ?1)",
        [&id],
    );
    id
}

/// 传输进度事件负载
#[derive(Clone, serde::Serialize)]
pub struct ProgressEvent {
    pub transfer_id: String,
    pub sent: u64,
    pub total: u64,
    pub status: String,
}

/// 接收进度事件负载（接收方分块落盘时发出）
#[derive(Clone, serde::Serialize)]
pub struct ReceiveProgressEvent {
    pub name: String,
    pub received: u64,
    pub total: u64,
    pub status: String,
}

/// 接收完成事件负载
#[derive(Clone, serde::Serialize)]
pub struct ReceivedEvent {
    pub kind: String,
    pub name: String,
    pub from: String,
    /// kind=text/clipboard 时为文本内容，文件/文件夹时为空
    pub text: String,
    /// 文件/文件夹落盘路径
    pub path: String,
}
