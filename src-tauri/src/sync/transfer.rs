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

struct TlsStream {
    sock: TcpStream,
    conn: Connection,
    peer_fp: String,
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
    let head = format!(
        "HTTP/1.1 {}\r\nContent-Length: {}\r\nContent-Type: text/plain; charset=utf-8\r\nConnection: close\r\n\r\n",
        status,
        body.len()
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

/// 等待接收方决策；同时监听发送端撤销和 5 分钟超时。
/// 返回 (选中的索引, 结果原因)：accepted / rejected / cancelled / timeout。
/// 原因随 407 响应体回传给发送端，用于显示「对方已拒绝接收」等具体文案。
fn wait_for_decision(
    rx: &mpsc::Receiver<String>,
    canceled: &Arc<AtomicBool>,
    count: usize,
) -> (Vec<usize>, &'static str) {
    let deadline = Instant::now() + Duration::from_secs(REQUEST_TIMEOUT_SECS);
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
fn trust_after_accept(rt: &SyncRuntime, peer_fp: &str, from: &str, from_name: &str, trusted: bool) {
    if trusted || peer_fp.is_empty() {
        return;
    }
    commands::trust_fingerprint(peer_fp, from, from_name);
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
    let result = match read_request(&mut stream)? {
        Some(req) => handle_request(&mut stream, req, &rt, &peer_fp, &peer_ip),
        None => Ok(()),
    };
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
            &kind, &display_name, total, count, &items,
        );
        let decision = wait_for_decision(&rx, &canceled, count);
        if let Ok(mut pending) = rt.pending.lock() {
            pending.remove(&request_id);
        }
        decision
    };

    if selected.is_empty() {
        return write_response(stream, "407 Rejected", reason.as_bytes());
    }
    trust_after_accept(rt, peer_fp, &from, &from_name, trusted);
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
            &kind, &name, size, count, &items,
        );

        let (selected, reason) = wait_for_decision(&rx, &canceled, count);
        if let Ok(mut pending) = rt.pending.lock() {
            pending.remove(&request_id);
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
        commands::trust_fingerprint(peer_fp, &from, &from_name);
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
    for chunk in data.chunks(64 * 1024) {
        file.write_all(chunk).map_err(|e| e.to_string())?;
        written += chunk.len() as u64;
        commands::emit_receive_progress(&rel, written, total, "receiving");
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
) -> Result<(String, String, String), String> {
    probe_inner(ip, port, identity, self_id, self_name, 3, CLIENT_READ_TIMEOUT_SECS)
}

/// 网段扫描专用探测：连接/读都设短超时，避免逐个 IP 扫描时长时间卡住。
pub fn probe_scan(
    ip: &str,
    port: u16,
    identity: &Identity,
    self_id: &str,
    self_name: &str,
) -> Result<(String, String, String), String> {
    probe_inner(ip, port, identity, self_id, self_name, 1, 3)
}

fn probe_inner(
    ip: &str,
    port: u16,
    identity: &Identity,
    self_id: &str,
    self_name: &str,
    connect_secs: u64,
    read_secs: u64,
) -> Result<(String, String, String), String> {
    let mut stream = connect_tls_with(ip, port, identity, connect_secs, read_secs)?;
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
    Ok((json_field(body, "id"), json_field(body, "name"), json_field(body, "fingerprint")))
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
    connect_tls_with(ip, port, identity, 3, CLIENT_READ_TIMEOUT_SECS)
}

/// 可指定超时的 mTLS 连接。网段扫描用短超时，避免扫到异常端口时卡死扫描线程。
fn connect_tls_with(
    ip: &str,
    port: u16,
    identity: &Identity,
    connect_secs: u64,
    read_secs: u64,
) -> Result<TlsStream, String> {
    let addr = format!("{}:{}", ip, port)
        .to_socket_addrs()
        .map_err(|e| e.to_string())?
        .next()
        .ok_or_else(|| "地址解析失败".to_string())?;
    let sock = TcpStream::connect_timeout(&addr, Duration::from_secs(connect_secs))
        .map_err(|e| format!("连接失败: {}", e))?;
    sock.set_read_timeout(Some(Duration::from_secs(read_secs))).ok();
    sock.set_write_timeout(Some(Duration::from_secs(read_secs))).ok();
    let config = identity::client_config(identity)?;
    let conn = rustls::ClientConnection::new(
        Arc::new(config),
        rustls::pki_types::ServerName::try_from("runhelp.local").map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("创建 TLS 客户端连接失败: {}", e))?;
    let mut stream = TlsStream { sock, conn: Connection::Client(conn), peer_fp: String::new() };
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
    if status.contains(" 200 ") {
        Ok(response_body)
    } else if status.contains(" 403 ") {
        Err("对方尚未信任本机，请先在对方设备确认配对".to_string())
    } else if status.contains(" 407 ") {
        // 响应体区分「明确拒绝 / 发送端撤销 / 超时掉线」，让发送方看到具体原因
        let msg = match response_body.as_str() {
            "cancelled" => "发送已撤销",
            "timeout" => "对方未在时限内接收（可能已离线）",
            _ => "对方已拒绝接收",
        };
        Err(msg.to_string())
    } else {
        Err(format!("传输失败: {}", status))
    }
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

/// 发送单个文件；preapproved=true 表示清单阶段已经勾选，不再弹确认。
pub fn send_file(
    ip: &str,
    port: u16,
    identity: &Identity,
    from_id: &str,
    from_name: &str,
    rel_path: &str,
    data: &[u8],
    total_count: usize,
    transfer_id: &str,
    preapproved: bool,
) -> Result<(), String> {
    let mut stream = connect_tls(ip, port, identity)?;
    let mut headers = common_headers(identity, from_id, from_name, "file", transfer_id);
    headers.push(format!("X-RH-RelPath: {}", url_encode(rel_path)));
    headers.push(format!("X-RH-Size: {}", data.len()));
    headers.push(format!("X-RH-Count: {}", total_count));
    headers.push(format!("X-RH-Preapproved: {}", if preapproved { 1 } else { 0 }));
    headers.push(format!("Content-Length: {}", data.len()));
    http_exchange(&mut stream, "/file", &headers, data)
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
