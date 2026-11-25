//! 同步传输：内置 mTLS 服务端（接收）+ 轻量 mTLS 客户端（发送）。
//! 两端互为服务端，因此 A→B 与 B→A 都走同一套接口。
//!
//! TLS：自签证书 + 指纹固定（certificate pinning）。
//!   - 服务端要求客户端证书指纹在 trusted_fps 里（TOFU 后写入）。
//!   - 客户端校验服务端证书指纹 == 目标设备记录的指纹。
//! HTTP（跑在 TLS 之上，自解析，避免引入 tokio/axum）：
//!   GET  /hello  -> JSON {id,name,platform,fingerprint}
//!   POST /text   X-RH-From / X-RH-Name / X-RH-Kind / X-RH-Msg(encoded)
//!   POST /file   X-RH-From / X-RH-Name / X-RH-Kind / X-RH-RelPath(encoded) /
//!                X-RH-Size / X-RH-Count / X-RH-Msg(encoded)
//!   body = 原始字节
//! 返回 200 已接收 / 403 证书未信任 / 407 用户拒绝 / 500 其它错误。

use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use rustls::Connection;

use super::commands;
use super::identity::{self, Identity};
use super::{
    PendingItem, PendingRequest, ReceivedEvent, SyncRuntime, CLIENT_READ_TIMEOUT_SECS,
    REQUEST_TIMEOUT_SECS, TRANSFER_PORT,
};

// ===== TLS 流包装：让 rustls 连接像普通 TcpStream 一样 Read/Write =====

/// 单条 keep-alive 连接上最多处理的请求数，防对端异常时长期占用线程。
const MAX_PIPELINED_REQUESTS: usize = 4096;

/// 批量发送时读文件的块大小（也是进度回调粒度）。
const SEND_CHUNK: usize = 128 * 1024;

struct TlsStream {
    sock: TcpStream,
    conn: Connection,
    peer_fp: String,
    /// 本次响应是否保持连接（HTTP/1.1 keep-alive）。
    /// 批量发文件时一条 TLS 连接上连续发多个请求，省掉每个文件一次握手，
    /// 这是「发文件夹时状态页持续卡顿」的主因（握手非常吃 CPU）。
    keep_alive: bool,
}

impl TlsStream {
    fn peer_fingerprint(&self) -> &str {
        &self.peer_fp
    }

    /// 发送 TLS 关闭通知，避免对端 read_to_end 遇到不完整关闭。
    fn close_notify(&mut self) {
        let _ = self.conn.send_close_notify();
        let _ = self.conn.write_tls(&mut self.sock);
        let _ = self.sock.flush();
    }

    /// 完成握手（读写 TLS 记录直到不再 handshaking），并提取对端证书指纹。
    fn handshake(&mut self) -> Result<(), String> {
        loop {
            if self.conn.wants_write() {
                self.conn
                    .write_tls(&mut self.sock)
                    .map_err(|e| format!("TLS 握手写入失败: {}", e))?;
            }
            if !self.conn.is_handshaking() {
                break;
            }
            if self.conn.wants_read() {
                let n = self
                    .conn
                    .read_tls(&mut self.sock)
                    .map_err(|e| format!("TLS 握手读取失败: {}", e))?;
                if n == 0 {
                    return Err("TLS 握手被对端中断".to_string());
                }
                self.conn
                    .process_new_packets()
                    .map_err(|e| format!("TLS 握手失败: {}", e))?;
            }
        }
        if let Some(certs) = self.conn.peer_certificates() {
            if let Some(cert) = certs.first() {
                self.peer_fp = identity::fingerprint_of(cert);
            }
        }
        Ok(())
    }

}

impl Read for TlsStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            // 明文缓冲为空时 Reader::read 会返回 WouldBlock，此时先从 socket 读密文喂给 rustls。
            match self.conn.reader().read(buf) {
                Ok(0) => {
                    if !self.conn.wants_read() {
                        return Ok(0);
                    }
                }
                Ok(n) => return Ok(n),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                Err(e) => return Err(e),
            }
            let got = self.conn.read_tls(&mut self.sock)?;
            if got == 0 {
                return Ok(0);
            }
            self.conn
                .process_new_packets()
                .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        }
    }
}

impl Write for TlsStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.conn.writer().write(buf)?;
        self.conn.write_tls(&mut self.sock)?;
        self.sock.flush()?;
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.conn.writer().flush()?;
        self.conn.write_tls(&mut self.sock)?;
        self.sock.flush()
    }
}

// ===== 极简 HTTP 解析（跑在 TLS 之上） =====

struct HttpRequest {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl HttpRequest {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// 从 TLS 流读一个完整 HTTP 请求（读到 Content-Length 长度的 body）。
fn read_request(stream: &mut TlsStream) -> Result<Option<HttpRequest>, String> {
    // 读请求行（以 \r\n 结束）
    let mut line = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        match stream.read(&mut byte) {
            Ok(0) => return Ok(None),
            Ok(_) => {
                if byte[0] == b'\n' {
                    break;
                }
                if byte[0] != b'\r' {
                    line.push(byte[0]);
                }
            }
            Err(e) => return Err(format!("读取请求行失败: {}", e)),
        }
    }
    let request_line = String::from_utf8_lossy(&line);
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("").to_string();
    if method.is_empty() || path.is_empty() {
        return Ok(None);
    }

    // 读头部
    let mut headers: Vec<(String, String)> = Vec::new();
    loop {
        let mut hline = Vec::new();
        loop {
            match stream.read(&mut byte) {
                Ok(0) => return Ok(None),
                Ok(_) => {
                    if byte[0] == b'\n' {
                        break;
                    }
                    if byte[0] != b'\r' {
                        hline.push(byte[0]);
                    }
                }
                Err(e) => return Err(format!("读取请求头失败: {}", e)),
            }
        }
        if hline.is_empty() {
            break;
        }
        let text = String::from_utf8_lossy(&hline);
        if let Some((k, v)) = text.split_once(':') {
            headers.push((k.trim().to_string(), v.trim().to_string()));
        }
    }

    let content_len: usize = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("Content-Length"))
        .and_then(|(_, v)| v.parse().ok())
        .unwrap_or(0);

    let mut body = vec![0u8; content_len];
    if content_len > 0 {
        stream.read_exact(&mut body).map_err(|e| format!("读取请求体失败: {}", e))?;
    }

    Ok(Some(HttpRequest { method, path, headers, body }))
}

fn write_response(stream: &mut TlsStream, status: &str, body: &[u8]) -> Result<(), String> {
    let connection = if stream.keep_alive { "keep-alive" } else { "close" };
    let head = format!(
        "HTTP/1.1 {}\r\nContent-Length: {}\r\nContent-Type: text/plain; charset=utf-8\r\nConnection: {}\r\n\r\n",
        status,
        body.len(),
        connection,
    );
    stream.write_all(head.as_bytes()).map_err(|e| e.to_string())?;
    stream.write_all(body).map_err(|e| e.to_string())?;
    stream.flush().map_err(|e| e.to_string())
}

// ===== URL 编解码 =====

fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.as_bytes() {
        match *b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(*b as char),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

fn url_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

fn json_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// HTTP 头不允许出现换行，避免设备名等动态内容破坏请求。
fn header_safe(s: &str) -> String {
    s.replace('\r', "").replace('\n', "")
}

/// 接收方决策：all / none / "0,2,3" -> 实际接收索引。
fn parse_selection(decision: &str, count: usize) -> Vec<usize> {
    let value = decision.trim();
    if value.eq_ignore_ascii_case("all") {
        return (0..count).collect();
    }
    if value.is_empty() || value.eq_ignore_ascii_case("none") {
        return Vec::new();
    }
    value
        .split(',')
        .filter_map(|p| p.trim().parse::<usize>().ok())
        .filter(|i| *i < count)
        .collect()
}

/// 对端 TCP 连接是否还在：等待决策期间对端不会发任何数据，
/// 因此 peek 到 0 字节（FIN）或有数据（close_notify）都视为已断开。
fn socket_connected(sock: &TcpStream) -> bool {
    if sock.set_nonblocking(true).is_err() {
        return true;
    }
    let alive = match sock.peek(&mut [0u8; 1]) {
        Ok(0) => false,
        // 对端主动关闭会先发 TLS close_notify，缓冲区里能看到残留字节
        Ok(_) => false,
        Err(e) => !matches!(
            e.kind(),
            io::ErrorKind::ConnectionReset | io::ErrorKind::ConnectionAborted | io::ErrorKind::BrokenPipe
        ),
    };
    let _ = sock.set_nonblocking(false);
    alive
}

/// 发现列表里该指纹的设备是否已判定离线（掉 WiFi / 退出应用后由广播超时判定）。
fn device_gone(rt: &SyncRuntime, peer_fp: &str) -> bool {
    if peer_fp.is_empty() {
        return false;
    }
    if let Ok(devices) = rt.devices.lock() {
        if let Some(d) = devices.values().find(|d| d.fingerprint == peer_fp) {
            return !d.is_online(super::now_secs());
        }
    }
    false
}

/// 等待接收方决策；同时监听发送端撤销、发送端掉线和 5 分钟超时。
/// 返回 (选中的索引, 结果原因)：accepted / rejected / cancelled / timeout / offline。
/// 原因随 407 响应体回传给发送端，用于显示「对方已拒绝接收」等具体文案。
fn wait_for_decision(
    rx: &mpsc::Receiver<String>,
    canceled: &Arc<AtomicBool>,
    count: usize,
    peer_alive: &dyn Fn() -> bool,
) -> (Vec<usize>, &'static str) {
    let deadline = Instant::now() + Duration::from_secs(REQUEST_TIMEOUT_SECS);
    let mut gone_hits = 0u8;
    loop {
        if canceled.load(Ordering::SeqCst) {
            return (Vec::new(), "cancelled");
        }
        match rx.recv_timeout(Duration::from_millis(300)) {
            Ok(decision) => {
                let selected = parse_selection(&decision, count);
                let reason = if selected.is_empty() { "rejected" } else { "accepted" };
                return (selected, reason);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return (Vec::new(), "timeout"),
        }
        // 发送方中途掉线（退出应用 / 断网）：连续两次判定才结束，避免一次抖动误伤
        if peer_alive() {
            gone_hits = 0;
        } else {
            gone_hits += 1;
            if gone_hits >= 2 {
                return (Vec::new(), "offline");
            }
        }
        if Instant::now() >= deadline {
            return (Vec::new(), "timeout");
        }
    }
}

/// 从请求体解析文件清单。
fn parse_items(req: &HttpRequest) -> Vec<PendingItem> {
    let values: Vec<serde_json::Value> = serde_json::from_slice(&req.body).unwrap_or_default();
    let mut items: Vec<PendingItem> = values
        .iter()
        .map(|v| PendingItem {
            name: v.get("name").and_then(|x| x.as_str()).unwrap_or("").to_string(),
            size: v.get("size").and_then(|x| x.as_u64()).unwrap_or(0),
        })
        .filter(|i| !i.name.is_empty())
        .collect();
    if items.is_empty() {
        // 兼容旧版：没有清单时按单文件处理
        let name = url_decode(req.header("X-RH-RelPath").unwrap_or(""));
        if !name.is_empty() {
            items.push(PendingItem {
                name,
                size: req.header("X-RH-Size").and_then(|v| v.parse().ok()).unwrap_or(0),
            });
        }
    }
    items
}

/// TOFU：接收方接受后记住发送方指纹（仅内存事件，不做弹窗数据存储）。
fn trust_after_accept(
    rt: &SyncRuntime,
    peer_fp: &str,
    from: &str,
    from_name: &str,
    platform: &str,
    trusted: bool,
) {
    if trusted || peer_fp.is_empty() {
        return;
    }
    commands::trust_fingerprint_ex(peer_fp, from, from_name, platform);
    if let Ok(mut fps) = rt.trusted_fps.lock() {
        fps.insert(peer_fp.to_string());
    }
}

// ===== 服务端 =====

/// 启动 mTLS 接收服务。
pub fn start_server(rt: Arc<SyncRuntime>) -> Result<(), String> {
    let listener = TcpListener::bind(("0.0.0.0", TRANSFER_PORT))
        .map_err(|e| format!("绑定传输端口 {} 失败: {}", TRANSFER_PORT, e))?;
    let server_config = identity::server_config(&rt.identity)?;

    std::thread::Builder::new()
        .name("runhelp-sync-server".to_string())
        .spawn(move || {
            for stream in listener.incoming() {
                if rt.server_stop.load(Ordering::SeqCst) {
                    break;
                }
                let sock = match stream {
                    Ok(s) => s,
                    Err(_) => continue,
                };
                let _ = sock.set_read_timeout(Some(Duration::from_secs(120)));
                let _ = sock.set_write_timeout(Some(Duration::from_secs(30)));
                let config = server_config.clone();
                let rt = rt.clone();
                std::thread::spawn(move || {
                    let _ = handle_connection(sock, config, rt);
                });
            }
        })
        .map_err(|e| format!("启动传输服务线程失败: {}", e))?;
    Ok(())
}

fn handle_connection(sock: TcpStream, config: rustls::ServerConfig, rt: Arc<SyncRuntime>) -> Result<(), String> {
    let conn = rustls::ServerConnection::new(Arc::new(config))
        .map_err(|e| format!("创建 TLS 服务端连接失败: {}", e))?;
    let mut stream = TlsStream {
        sock,
        conn: Connection::Server(conn),
        peer_fp: String::new(),
        keep_alive: false,
    };
    stream.handshake()?;
    let peer_fp = stream.peer_fingerprint().to_string();
    // 对端 IP（不带端口）：用于 /hello 反向登记探测方。
    let peer_ip = stream
        .sock
        .peer_addr()
        .ok()
        .map(|a| a.ip().to_string())
        .unwrap_or_default();
    // 一条连接上可以连续处理多个 /file 请求（发送端批量发送时复用连接）。
    // 其它端点（/hello、/manifest、/text、/cancel）语义上都是一问一答，处理完即关。
    let mut result = Ok(());
    for _ in 0..MAX_PIPELINED_REQUESTS {
        let req = match read_request(&mut stream) {
            Ok(Some(req)) => req,
            Ok(None) => break,
            Err(e) => {
                result = Err(e);
                break;
            }
        };
        let wants_keep = req.path == "/file"
            && req
                .header("Connection")
                .map(|v| !v.eq_ignore_ascii_case("close"))
                .unwrap_or(false);
        stream.keep_alive = wants_keep;
        result = handle_request(&mut stream, req, &rt, &peer_fp, &peer_ip);
        if result.is_err() || !wants_keep {
            break;
        }
    }
    stream.keep_alive = false;
    stream.close_notify();
    result
}

/// 文件/文件夹批次清单：接收方一次展示全部文件，返回被勾选的索引。
fn handle_manifest(
    stream: &mut TlsStream,
    req: HttpRequest,
    rt: &Arc<SyncRuntime>,
    peer_fp: &str,
) -> Result<(), String> {
    let from = header_safe(req.header("X-RH-From").unwrap_or(""));
    let from_name = url_decode(req.header("X-RH-Name").unwrap_or(""));
    let kind = header_safe(req.header("X-RH-Kind").unwrap_or("file"));
    let claimed_fp = header_safe(req.header("X-RH-Fingerprint").unwrap_or(""));
    let transfer_id = header_safe(req.header("X-RH-Transfer").unwrap_or(""));
    let platform = header_safe(req.header("X-RH-Platform").unwrap_or(""));
    if from.is_empty() || from_name.is_empty() || transfer_id.is_empty() {
        return write_response(stream, "400 Bad Request", b"missing headers");
    }
    if claimed_fp != peer_fp {
        return write_response(stream, "403 Fingerprint Mismatch", b"fingerprint mismatch");
    }

    let items = parse_items(&req);
    if items.is_empty() {
        return write_response(stream, "400 Bad Request", b"empty manifest");
    }
    let count = items.len();
    let total: u64 = items.iter().map(|i| i.size).sum();
    let display_name = if kind == "folder" {
        format!("{} · {} {}", items[0].name.split('/').next().unwrap_or("文件夹"), count, "个文件")
    } else {
        format!("{} {}", count, "个文件")
    };

    let trusted = commands::is_trusted_fingerprint(peer_fp);
    let auto = trusted && commands::auto_accept_fingerprint(peer_fp);
    let (selected, reason) = if auto {
        ((0..count).collect::<Vec<_>>(), "accepted")
    } else {
        let request_id = rt.next_request_id();
        let (tx, rx) = mpsc::channel();
        let canceled = Arc::new(AtomicBool::new(false));
        {
            let mut pending = rt.pending.lock().map_err(|e| e.to_string())?;
            pending.insert(
                request_id.clone(),
                PendingRequest {
                    id: request_id.clone(),
                    transfer_id: transfer_id.clone(),
                    from_id: from.clone(),
                    from_name: from_name.clone(),
                    from_platform: platform.clone(),
                    from_fp: peer_fp.to_string(),
                    kind: kind.clone(),
                    name: display_name.clone(),
                    size: total,
                    count,
                    items: items.clone(),
                    respond: tx,
                    canceled: canceled.clone(),
                },
            );
        }
        commands::emit_request(
            &request_id, &transfer_id, &from_name, &platform, peer_fp, trusted,
            &kind, &display_name, total, count,
        );
        let decision = {
            let alive = || socket_connected(&stream.sock) && !device_gone(rt, peer_fp);
            wait_for_decision(&rx, &canceled, count, &alive)
        };
        if let Ok(mut pending) = rt.pending.lock() {
            pending.remove(&request_id);
        }
        // 超时或发送端掉线时通知接收方 UI 收尾，
        // 否则接收页会一直显示「待接收」，看起来像传输卡住。
        if decision.1 == "timeout" || decision.1 == "offline" {
            commands::emit_request_timeout(&transfer_id, decision.1);
        }
        decision
    };

    if selected.is_empty() {
        return write_response(stream, "407 Rejected", reason.as_bytes());
    }
    trust_after_accept(rt, peer_fp, &from, &from_name, &platform, trusted);
    let body = selected.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(",");
    write_response(stream, "200 OK", body.as_bytes())
}

fn handle_request(stream: &mut TlsStream, req: HttpRequest, rt: &Arc<SyncRuntime>, peer_fp: &str, peer_ip: &str) -> Result<(), String> {
    let path = req.path.split('?').next().unwrap_or("");

    if req.method == "GET" && path == "/hello" {
        // 反向登记：对方探测我时带了身份头，就把它也加入我的附近列表。
        // 指纹用 mTLS 握手提取的 peer_fp（不可伪造），不采信声称的指纹。
        let from_id = header_safe(req.header("X-RH-Id").unwrap_or(""));
        if !from_id.is_empty() && !peer_fp.is_empty() && !peer_ip.is_empty() {
            let from_name = url_decode(req.header("X-RH-Name").unwrap_or(""));
            let from_platform = header_safe(req.header("X-RH-Platform").unwrap_or(""));
            super::discovery::upsert_device_probe(
                rt,
                from_id,
                if from_name.is_empty() { "未知设备".to_string() } else { from_name },
                if from_platform.is_empty() { "unknown".to_string() } else { from_platform },
                peer_ip.to_string(),
                super::TRANSFER_PORT,
                peer_fp.to_string(),
            );
        }
        let (id, name, fp) = {
            let conn = crate::state::AppState::open_db_for_sync();
            let id = conn.as_ref().map(|c| super::device_id(c)).unwrap_or_default();
            let name = conn.as_ref().map(|c| super::display_name(c)).unwrap_or_default();
            (id, name, rt.identity.fingerprint.clone())
        };
        let body = format!(
            "{{\"id\":\"{}\",\"name\":\"{}\",\"platform\":\"{}\",\"fingerprint\":\"{}\"}}",
            json_escape(&id),
            json_escape(&name),
            json_escape(std::env::consts::OS),
            fp
        );
        return write_response(stream, "200 OK", body.as_bytes());
    }

    // 发送端主动撤销：只作用于当前内存中的待确认请求，不写历史。
    if req.method == "POST" && path == "/cancel" {
        let transfer_id = header_safe(req.header("X-RH-Transfer").unwrap_or(""));
        if !transfer_id.is_empty() {
            let mut matched = false;
            if let Ok(pending) = rt.pending.lock() {
                for req_item in pending.values() {
                    if req_item.transfer_id == transfer_id {
                        matched = true;
                        req_item.canceled.store(true, Ordering::SeqCst);
                        let _ = req_item.respond.send("none".to_string());
                    }
                }
            }
            // 通知接收方 UI 关掉对应请求的弹窗 / 待接收项（不要持有 pending 锁时 emit）。
            if matched {
                commands::emit_request_cancelled(&transfer_id);
            }
        }
        return write_response(stream, "200 OK", b"ok");
    }

    // 文件批次先发清单：接收方一次看到全部文件，可整批接收或按项勾选。
    if req.method == "POST" && path == "/manifest" {
        return handle_manifest(stream, req, rt, peer_fp);
    }

    if req.method != "POST" || (path != "/text" && path != "/file") {
        return write_response(stream, "404 Not Found", b"not found");
    }

    let from = header_safe(req.header("X-RH-From").unwrap_or(""));
    let from_name = url_decode(req.header("X-RH-Name").unwrap_or(""));
    let kind = header_safe(req.header("X-RH-Kind").unwrap_or(""));
    let msg = url_decode(req.header("X-RH-Msg").unwrap_or(""));
    let claimed_fp = header_safe(req.header("X-RH-Fingerprint").unwrap_or(""));
    let transfer_id = header_safe(req.header("X-RH-Transfer").unwrap_or(""));
    let platform = header_safe(req.header("X-RH-Platform").unwrap_or(""));
    let preapproved = req.header("X-RH-Preapproved").unwrap_or("") == "1";

    if from.is_empty() || from_name.is_empty() {
        return write_response(stream, "400 Bad Request", b"missing headers");
    }

    // 应用层身份互验：声明指纹必须 == 握手拿到的证书指纹，防止伪造 device_id。
    if claimed_fp != peer_fp {
        return write_response(stream, "403 Fingerprint Mismatch", b"fingerprint mismatch");
    }

    // 按指纹判断可信/自动接收（TOFU 首次确认后写入 trusted 集合）。
    let trusted = commands::is_trusted_fingerprint(peer_fp);
    let auto = trusted && commands::auto_accept_fingerprint(peer_fp);
    if !auto && !preapproved {
        let request_id = rt.next_request_id();
        let (tx, rx) = mpsc::channel();
        let canceled = Arc::new(AtomicBool::new(false));
        let size: u64 = req.header("X-RH-Size").and_then(|v| v.parse().ok()).unwrap_or(req.body.len() as u64);
        let count: usize = req.header("X-RH-Count").and_then(|v| v.parse().ok()).unwrap_or(1);
        let name = if path == "/file" {
            url_decode(req.header("X-RH-RelPath").unwrap_or(""))
        } else {
            msg.clone()
        };
        let items = vec![PendingItem { name: name.clone(), size }];
        {
            if let Ok(mut pending) = rt.pending.lock() {
                pending.insert(
                    request_id.clone(),
                    PendingRequest {
                        id: request_id.clone(),
                        transfer_id: transfer_id.clone(),
                        from_id: from.clone(),
                        from_name: from_name.clone(),
                        from_platform: platform.clone(),
                        from_fp: peer_fp.to_string(),
                        kind: kind.clone(),
                        name: name.clone(),
                        size,
                        count,
                        items: items.clone(),
                        respond: tx,
                        canceled: canceled.clone(),
                    },
                );
            }
        }
        commands::emit_request(
            &request_id, &transfer_id, &from_name, &platform, peer_fp, trusted,
            &kind, &name, size, count,
        );

        let (selected, reason) = {
            let alive = || socket_connected(&stream.sock) && !device_gone(rt, peer_fp);
            wait_for_decision(&rx, &canceled, count, &alive)
        };
        if let Ok(mut pending) = rt.pending.lock() {
            pending.remove(&request_id);
        }
        if reason == "timeout" || reason == "offline" {
            commands::emit_request_timeout(&transfer_id, reason);
        }
        if selected.is_empty() {
            return write_response(stream, "407 Rejected", reason.as_bytes());
        }
    }

    let result = if path == "/text" {
        let text = String::from_utf8_lossy(&req.body).to_string();
        commands::emit_received(ReceivedEvent {
            kind: kind.clone(),
            name: msg.clone(),
            from: from_name.clone(),
            text: text.clone(),
            path: String::new(),
        });
        commands::record_receive(&kind, &msg, &from_name, text.len() as u64);
        Ok(())
    } else {
        let rel_path = url_decode(req.header("X-RH-RelPath").unwrap_or(""));
        receive_file(&rel_path, &from_name, &req.body, rt, peer_fp)
    };

    // 首次传输成功后自动记住对方指纹为可信（TOFU）
    if !trusted {
        commands::trust_fingerprint_ex(peer_fp, &from, &from_name, &platform);
        if let Ok(mut fps) = rt.trusted_fps.lock() {
            fps.insert(peer_fp.to_string());
        }
    }

    match result {
        Ok(()) => write_response(stream, "200 OK", b"ok"),
        Err(e) => write_response(stream, "500 Internal Server Error", e.as_bytes()),
    }
}

/// 落盘文件（body 已是完整字节；文件按相对路径重建）。
fn receive_file(
    rel_path: &str,
    from_name: &str,
    data: &[u8],
    rt: &Arc<SyncRuntime>,
    _peer_fp: &str,
) -> Result<(), String> {
    let base = commands::receive_dir();
    let rel = crate::platform::safe_relative_path(rel_path).ok_or_else(|| "illegal path".to_string())?;
    let target = base.join(&rel);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let mut file = std::fs::File::create(&target).map_err(|e| e.to_string())?;
    let total = data.len() as u64;
    let mut written = 0u64;
    // 进度节流：64KB 一块逐块上报会让前端每秒重渲染上百次（大文件直接卡死），
    // 这里按 200ms 合并上报，末块与首块必报，进度条依旧连贯。
    let mut last_emit = Instant::now();
    for chunk in data.chunks(64 * 1024) {
        file.write_all(chunk).map_err(|e| e.to_string())?;
        written += chunk.len() as u64;
        if last_emit.elapsed() >= Duration::from_millis(200) {
            commands::emit_receive_progress(&rel, written, total, "receiving");
            last_emit = Instant::now();
        }
    }
    drop(file);
    commands::emit_receive_progress(&rel, total, total, "done");
    let display = target.to_string_lossy().to_string();
    commands::emit_received(ReceivedEvent {
        kind: "file".to_string(),
        name: rel.clone(),
        from: from_name.to_string(),
        text: String::new(),
        path: display,
    });
    commands::record_receive("file", &rel, from_name, data.len() as u64);
    let _ = rt;
    Ok(())
}

// ===== 客户端 =====

/// 探测目标设备信息（含指纹）：宽松 TLS 握手，只取 /hello。
/// 请求头带上本机身份，服务端据此反向登记本机（一次探测双方互相认识，
/// 对齐 LocalSend /register 的双向发现效果）。失败时身份头缺失不影响探测本身。
pub fn probe(
    ip: &str,
    port: u16,
    identity: &Identity,
    self_id: &str,
    self_name: &str,
) -> Result<ProbeInfo, String> {
    probe_inner(ip, port, identity, self_id, self_name, 3_000, CLIENT_READ_TIMEOUT_SECS * 1_000)
}

/// 网段扫描专用探测：连接/读都设短超时，避免逐个 IP 扫描时长时间卡住。
pub fn probe_scan(
    ip: &str,
    port: u16,
    identity: &Identity,
    self_id: &str,
    self_name: &str,
) -> Result<ProbeInfo, String> {
    probe_inner(ip, port, identity, self_id, self_name, 1_000, 3_000)
}

/// 存活探测（近期设备专用）：**单次、极短超时**。
///
/// 这里曾经是「放宽超时 + 重试一次」：2s 连接 + 6s 读，两次加 350ms 退避，
/// 单台最坏 ≈ 16 秒。而所有设备的离线重算要等整轮 join 结束才做，
/// 只要列表里有一台手机被系统冻结（TCP 连得上但应用层不应答），
/// 整轮就被它拖到十几秒，于是「电脑端识别手机离线」变成 15~40 秒。
///
/// 现在改成一轮只给 ~0.9 秒：连接 0.6s / 读 0.9s，失败不重试——
/// 重试交给下一轮探测（2 秒后），单台卡死只浪费它自己那 0.9 秒。
pub fn probe_liveness(
    ip: &str,
    port: u16,
    identity: &Identity,
    self_id: &str,
    self_name: &str,
) -> Result<ProbeInfo, String> {
    probe_inner(ip, port, identity, self_id, self_name, 600, 900)
}

/// 手动刷新专用探测：用户主动点的，多等 1 秒完全可接受，换来更少的误判。
///
/// 手动刷新是「一次不应答即判离线」（strict），如果还用后台那套 600ms/900ms，
/// 手机 Wi-Fi 省电或刚好在传大文件时会握手超时，于是点一下刷新设备就变离线，
/// 两秒后后台探测成功又上线 —— 这正是用户看到的「刷新一下就闪」。
pub fn probe_manual(
    ip: &str,
    port: u16,
    identity: &Identity,
    self_id: &str,
    self_name: &str,
) -> Result<ProbeInfo, String> {
    probe_inner(ip, port, identity, self_id, self_name, 1_200, 1_500)
}

/// /hello 探测结果。platform 必须一起带回来，否则探测路径只能填 "unknown"，
/// 会出现「设备类型偶尔变成 unknown」（广播没到时被探测值覆盖）。
#[derive(Clone, Debug, Default)]
pub struct ProbeInfo {
    pub id: String,
    pub name: String,
    pub platform: String,
    pub fingerprint: String,
}

fn probe_inner(
    ip: &str,
    port: u16,
    identity: &Identity,
    self_id: &str,
    self_name: &str,
    connect_ms: u64,
    read_ms: u64,
) -> Result<ProbeInfo, String> {
    let mut stream = connect_tls_with(
        ip,
        port,
        identity,
        Duration::from_millis(connect_ms),
        Duration::from_millis(read_ms),
    )?;
    let request = format!(
        "GET /hello HTTP/1.1\r\nHost: {}\r\nConnection: close\r\nX-RH-Id: {}\r\nX-RH-Name: {}\r\nX-RH-Platform: {}\r\n\r\n",
        ip,
        header_safe(self_id),
        header_safe(&url_encode(self_name)),
        std::env::consts::OS,
    );
    stream.write_all(request.as_bytes()).map_err(|e| e.to_string())?;
    stream.flush().map_err(|e| e.to_string())?;
    let mut response = Vec::new();
    stream.read_to_end(&mut response).map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&response);
    let body = text.split("\r\n\r\n").nth(1).unwrap_or("").trim();
    Ok(ProbeInfo {
        id: json_field(body, "id"),
        name: json_field(body, "name"),
        platform: json_field(body, "platform"),
        fingerprint: json_field(body, "fingerprint"),
    })
}

fn json_field(body: &str, key: &str) -> String {
    let needle = format!("\"{}\"", key);
    body.find(&needle)
        .and_then(|i| {
            let rest = &body[i + needle.len()..];
            let rest = rest.split_once(':')?.1.trim_start();
            let rest = rest.strip_prefix('"')?;
            let end = rest.find('"')?;
            Some(rest[..end].to_string())
        })
        .unwrap_or_default()
}

/// 建立 mTLS 连接（宽松握手，身份在应用层按指纹判定）。
fn connect_tls(ip: &str, port: u16, identity: &Identity) -> Result<TlsStream, String> {
    // 等待对方点「接收/拒绝」最长 5 分钟；客户端超时要略大，否则会先于服务端断线。
    connect_tls_with(
        ip,
        port,
        identity,
        Duration::from_secs(3),
        Duration::from_secs(CLIENT_READ_TIMEOUT_SECS),
    )
}

/// 可指定超时的 mTLS 连接。网段扫描/存活探测用毫秒级短超时，避免扫到异常端口时卡死线程。
fn connect_tls_with(
    ip: &str,
    port: u16,
    identity: &Identity,
    connect_timeout: Duration,
    read_timeout: Duration,
) -> Result<TlsStream, String> {
    let addr = format!("{}:{}", ip, port)
        .to_socket_addrs()
        .map_err(|e| e.to_string())?
        .next()
        .ok_or_else(|| "地址解析失败".to_string())?;
    let sock = TcpStream::connect_timeout(&addr, connect_timeout)
        .map_err(|e| format!("连接失败: {}", e))?;
    sock.set_read_timeout(Some(read_timeout)).ok();
    sock.set_write_timeout(Some(read_timeout)).ok();
    let config = identity::client_config(identity)?;
    let conn = rustls::ClientConnection::new(
        Arc::new(config),
        rustls::pki_types::ServerName::try_from("runhelp.local").map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("创建 TLS 客户端连接失败: {}", e))?;
    let mut stream = TlsStream { sock, conn: Connection::Client(conn), peer_fp: String::new(), keep_alive: false };
    stream.handshake()?;
    Ok(stream)
}

fn http_exchange_body(
    stream: &mut TlsStream,
    path: &str,
    headers: &[String],
    body: &[u8],
) -> Result<String, String> {
    let mut head = format!("POST {} HTTP/1.1\r\nHost: runhelp.local\r\nConnection: close\r\n", path);
    for h in headers {
        head.push_str(h);
        head.push_str("\r\n");
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes()).map_err(|e| e.to_string())?;
    stream.write_all(body).map_err(|e| e.to_string())?;
    stream.flush().map_err(|e| e.to_string())?;
    let mut response = Vec::new();
    stream.read_to_end(&mut response).map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&response);
    let status = text.lines().next().unwrap_or("");
    let response_body = text.split("\r\n\r\n").nth(1).unwrap_or("").trim().to_string();
    classify_response(status, &response_body)
}

/// 把 HTTP 响应翻译成业务结果（200 成功 / 403 未信任 / 407 拒绝）。
fn classify_response(status: &str, response_body: &str) -> Result<String, String> {
    if status.contains(" 200 ") {
        Ok(response_body.to_string())
    } else if status.contains(" 403 ") {
        Err("对方尚未信任本机，请先在对方设备确认配对".to_string())
    } else if status.contains(" 407 ") {
        // 响应体区分「明确拒绝 / 发送端撤销 / 超时掉线」，让发送方看到具体原因
        let msg = match response_body {
            "cancelled" => "发送已撤销",
            "timeout" => "对方未在时限内接收（可能已离线）",
            _ => "对方已拒绝接收",
        };
        Err(msg.to_string())
    } else {
        let detail = if response_body.is_empty() {
            String::new()
        } else {
            format!(" - {}", response_body)
        };
        Err(format!("传输失败: {}{}", status, detail))
    }
}

/// 写请求头（body 由调用方自己写）。`keep_alive` 决定连接是否复用。
fn write_request_head(
    stream: &mut TlsStream,
    path: &str,
    headers: &[String],
    keep_alive: bool,
) -> Result<(), String> {
    let mut head = format!(
        "POST {} HTTP/1.1\r\nHost: runhelp.local\r\nConnection: {}\r\n",
        path,
        if keep_alive { "keep-alive" } else { "close" }
    );
    for h in headers {
        head.push_str(h);
        head.push_str("\r\n");
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes()).map_err(|e| e.to_string())
}

/// 读一行响应头（到 \n 为止）。对端关闭且没有内容时返回 None。
fn read_line(stream: &mut TlsStream) -> Result<Option<String>, String> {
    let mut line = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        match stream.read(&mut byte) {
            Ok(0) => {
                return Ok(if line.is_empty() {
                    None
                } else {
                    Some(String::from_utf8_lossy(&line).to_string())
                })
            }
            Ok(_) => {
                if byte[0] == b'\n' {
                    break;
                }
                if byte[0] != b'\r' {
                    line.push(byte[0]);
                }
            }
            Err(e) => return Err(format!("读取响应失败: {}", e)),
        }
    }
    Ok(Some(String::from_utf8_lossy(&line).to_string()))
}

/// 读一个响应。按 Content-Length 读 body，不依赖对端关闭连接——
/// keep-alive 连接上连续发多个请求时这是必需的（read_to_end 会一直等下去）。
/// 第三个返回值表示对端是否愿意保持连接（老版本服务端会回 `Connection: close`）。
fn read_response(stream: &mut TlsStream) -> Result<(String, String, bool), String> {
    let status = read_line(stream)?.ok_or_else(|| "对端已关闭连接".to_string())?;
    let mut content_len = 0usize;
    let mut keep = true;
    loop {
        let line = read_line(stream)?.ok_or_else(|| "对端提前关闭连接".to_string())?;
        if line.is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            let key = k.trim();
            if key.eq_ignore_ascii_case("Content-Length") {
                content_len = v.trim().parse().unwrap_or(0);
            } else if key.eq_ignore_ascii_case("Connection") {
                keep = !v.trim().eq_ignore_ascii_case("close");
            }
        }
    }
    let mut body = vec![0u8; content_len];
    if content_len > 0 {
        stream
            .read_exact(&mut body)
            .map_err(|e| format!("读取响应体失败: {}", e))?;
    }
    Ok((status, String::from_utf8_lossy(&body).to_string(), keep))
}

fn http_exchange(
    stream: &mut TlsStream,
    path: &str,
    headers: &[String],
    body: &[u8],
) -> Result<(), String> {
    http_exchange_body(stream, path, headers, body).map(|_| ())
}

fn common_headers(
    identity: &Identity,
    from_id: &str,
    from_name: &str,
    kind: &str,
    transfer_id: &str,
) -> Vec<String> {
    vec![
        format!("X-RH-From: {}", header_safe(from_id)),
        format!("X-RH-Name: {}", url_encode(from_name)),
        format!("X-RH-Kind: {}", header_safe(kind)),
        format!("X-RH-Fingerprint: {}", identity.fingerprint),
        format!("X-RH-Transfer: {}", header_safe(transfer_id)),
        format!("X-RH-Platform: {}", header_safe(std::env::consts::OS)),
    ]
}

/// 发送文字/剪贴板
pub fn send_text(
    ip: &str,
    port: u16,
    identity: &Identity,
    from_id: &str,
    from_name: &str,
    kind: &str,
    text: &str,
    transfer_id: &str,
) -> Result<(), String> {
    let mut stream = connect_tls(ip, port, identity)?;
    let mut headers = common_headers(identity, from_id, from_name, kind, transfer_id);
    headers.push(format!("X-RH-Msg: {}", url_encode(&text.chars().take(60).collect::<String>())));
    headers.push(format!("X-RH-Size: {}", text.as_bytes().len()));
    headers.push(format!("Content-Length: {}", text.as_bytes().len()));
    http_exchange(&mut stream, "/text", &headers, text.as_bytes())
}

/// 发送文件清单：接收方一次展示完整列表，返回勾选接收的索引。
pub fn send_manifest(
    ip: &str,
    port: u16,
    identity: &Identity,
    from_id: &str,
    from_name: &str,
    transfer_id: &str,
    items: &[PendingItem],
) -> Result<Vec<usize>, String> {
    let manifest: Vec<serde_json::Value> = items
        .iter()
        .map(|i| serde_json::json!({ "name": i.name, "size": i.size }))
        .collect();
    let body = serde_json::to_vec(&manifest).map_err(|e| e.to_string())?;
    let mut stream = connect_tls(ip, port, identity)?;
    let mut headers = common_headers(identity, from_id, from_name, "file", transfer_id);
    headers.push(format!("X-RH-Count: {}", items.len()));
    headers.push(format!("Content-Length: {}", body.len()));
    let response = http_exchange_body(&mut stream, "/manifest", &headers, &body)?;
    Ok(parse_selection(&response, items.len()))
}

/// 批量发送一组文件：**整批复用一条 TLS 连接**。
///
/// 为什么必须这么改：`send_file` 每个文件都新建连接并重新握手（mTLS = 双向证书 +
/// 签名验证），发一个几百个文件的文件夹就是几百次握手，CPU 被占满几秒，
/// 手机端表现为「状态页持续卡顿」。这里一次握手发完全部文件，顺带：
/// - 文件按 128KB 分块读取发送，不再 `fs::read` 整个文件进内存（大视频不再吃双份内存）；
/// - 每块回调一次进度，由调用方按时间节流后再发事件给前端。
///
/// 返回实际发送的总字节数。`on_progress(累计字节, 当前文件已发字节)`。
///
/// 兼容旧版本对端：老服务端每个请求回完就关连接（响应带 `Connection: close`），
/// 这时下一份文件自动重新握手，不会把「新旧混装」的传输搞断。
pub fn send_files_bulk<F>(
    ip: &str,
    port: u16,
    identity: &Identity,
    from_id: &str,
    from_name: &str,
    files: &[(String, std::path::PathBuf)],
    transfer_id: &str,
    cancel: &AtomicBool,
    on_progress: &mut F,
) -> Result<u64, String>
where
    F: FnMut(u64, u64),
{
    if files.is_empty() {
        return Ok(0);
    }
    let count = files.len();
    let mut stream: Option<TlsStream> = None;
    let mut buf = vec![0u8; SEND_CHUNK];
    let mut sent_total: u64 = 0;

    for (rel, path) in files {
        if cancel.load(Ordering::SeqCst) {
            return Err("发送已撤销".to_string());
        }
        let mut file = std::fs::File::open(path)
            .map_err(|e| format!("读取 {} 失败: {}", path.display(), e))?;
        let size = file.metadata().map(|m| m.len()).unwrap_or(0);
        let mut headers = common_headers(identity, from_id, from_name, "file", transfer_id);
        headers.push(format!("X-RH-RelPath: {}", url_encode(rel)));
        headers.push(format!("X-RH-Size: {}", size));
        headers.push(format!("X-RH-Count: {}", count));
        headers.push("X-RH-Preapproved: 1".to_string());
        headers.push(format!("Content-Length: {}", size));

        if stream.is_none() {
            let mut fresh = connect_tls(ip, port, identity)?;
            fresh.keep_alive = true;
            stream = Some(fresh);
        }
        let st = stream.as_mut().expect("stream just ensured");
        write_request_head(st, "/file", &headers, true)?;

        let mut file_sent: u64 = 0;
        loop {
            if cancel.load(Ordering::SeqCst) {
                return Err("发送已撤销".to_string());
            }
            let n = match file.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) => return Err(format!("读取 {} 失败: {}", path.display(), e)),
            };
            st.write_all(&buf[..n])
                .map_err(|e| format!("发送 {} 失败: {}", rel, e))?;
            file_sent += n as u64;
            sent_total += n as u64;
            on_progress(sent_total, file_sent);
        }
        st.flush().map_err(|e| e.to_string())?;
        let (status, body, keep) = read_response(st)?;
        classify_response(&status, &body)?;
        if !keep {
            if let Some(mut used) = stream.take() {
                used.close_notify();
            }
        }
    }

    if let Some(mut s) = stream.take() {
        s.keep_alive = false;
        s.close_notify();
    }
    Ok(sent_total)
}

/// 通知对端撤销本次传输。
pub fn send_cancel(ip: &str, port: u16, identity: &Identity, transfer_id: &str) -> Result<(), String> {
    let mut stream = connect_tls(ip, port, identity)?;
    let headers = vec![
        format!("X-RH-From: {}", header_safe(&identity.fingerprint)),
        format!("X-RH-Fingerprint: {}", identity.fingerprint),
        format!("X-RH-Transfer: {}", header_safe(transfer_id)),
        format!("Content-Length: 0"),
    ];
    http_exchange(&mut stream, "/cancel", &headers, &[])
}
