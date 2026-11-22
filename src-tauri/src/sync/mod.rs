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
/// 设备多久没广播就判定离线（秒）。每秒广播一次，5 秒约等于连续丢 5 包，兼顾灵敏与抗抖动。
pub const DEVICE_TIMEOUT_SECS: i64 = 5;
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
    /// 最近一次收到广播的 Unix 秒
    pub last_seen: i64,
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
}

/// 待接收清单中的单项（文件/文字）
#[derive(Clone, Debug, serde::Serialize)]
pub struct PendingItem {
    pub name: String,
    pub size: u64,
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
