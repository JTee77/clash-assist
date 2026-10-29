//! CLI `serve`：127.0.0.1 上的最小 loopback HTTP。
//!
//! - 只绑定 127.0.0.1（绝不 0.0.0.0）
//! - 随机 token；每个请求需 `Authorization: Bearer <token>` 或 `X-Egress-Token: <token>`
//! - GET /health 在 Rust 侧直接回 `{ok:true}`
//! - 其它 POST /v1/* 经通道交给隐藏 WebView 的 `dispatchCli`，再回 CliEnvelope
//! - 请求串行：同一时间只处理一个 WebView 任务（并发连接会排队）

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

/// 默认端口（文档与帮助文案同步）。
#[allow(dead_code)] // 与 TS CLI_SERVE_DEFAULT_PORT / 文档同步
pub const DEFAULT_PORT: u16 = 17890;
/// 只允许 loopback。
pub const BIND_HOST: &str = "127.0.0.1";

const MAX_HEADER_BYTES: usize = 64 * 1024;
const MAX_BODY_BYTES: usize = 256 * 1024;
/// WebView 深测（check all）可能很久；HTTP 侧最多等这么久。
const JOB_WAIT_SECS: u64 = 600;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServeRoute {
    Health,
    Discover,
    Nodes,
    Gate,
    Env,
    CheckCurrent,
    CheckAll,
    CheckNode,
}

impl ServeRoute {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Health => "health",
            Self::Discover => "discover",
            Self::Nodes => "nodes",
            Self::Gate => "gate",
            Self::Env => "env",
            Self::CheckCurrent => "check/current",
            Self::CheckAll => "check/all",
            Self::CheckNode => "check/node",
        }
    }

    /// 是否需要交给 WebView（health 在 Rust 直接回）。
    #[allow(dead_code)]
    pub fn needs_webview(self) -> bool {
        !matches!(self, Self::Health)
    }
}

/// 路径路由（不含 query）。未知 → None。
pub fn match_route(method: &str, path: &str) -> Option<ServeRoute> {
    let path = path.split('?').next().unwrap_or(path);
    match (method, path) {
        ("GET", "/health") => Some(ServeRoute::Health),
        ("POST", "/v1/discover") => Some(ServeRoute::Discover),
        ("POST", "/v1/nodes") => Some(ServeRoute::Nodes),
        ("POST", "/v1/gate") => Some(ServeRoute::Gate),
        ("POST", "/v1/env") => Some(ServeRoute::Env),
        ("POST", "/v1/check/current") => Some(ServeRoute::CheckCurrent),
        ("POST", "/v1/check/all") => Some(ServeRoute::CheckAll),
        ("POST", "/v1/check/node") => Some(ServeRoute::CheckNode),
        _ => None,
    }
}

/// 从原始请求头里取 token（Authorization Bearer 或 X-Egress-Token）。
pub fn extract_token(headers: &HashMap<String, String>) -> Option<String> {
    if let Some(auth) = headers.get("authorization") {
        let auth = auth.trim();
        if let Some(rest) = auth
            .strip_prefix("Bearer ")
            .or_else(|| auth.strip_prefix("bearer "))
        {
            let t = rest.trim();
            if !t.is_empty() {
                return Some(t.to_string());
            }
        }
    }
    headers
        .get("x-egress-token")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

pub fn token_matches(headers: &HashMap<String, String>, expected: &str) -> bool {
    extract_token(headers).is_some_and(|t| t == expected)
}

/// 生成 URL-safe 随机 token（/dev/urandom，48 hex 字符）。
pub fn generate_token() -> Result<String, String> {
    let mut buf = [0u8; 24];
    fill_random(&mut buf)?;
    Ok(buf.iter().map(|b| format!("{b:02x}")).collect())
}

fn fill_random(buf: &mut [u8]) -> Result<(), String> {
    use std::fs::File;
    use std::io::Read as _;
    let mut f = File::open("/dev/urandom").map_err(|e| format!("打开 /dev/urandom 失败：{e}"))?;
    f.read_exact(buf)
        .map_err(|e| format!("读取 /dev/urandom 失败：{e}"))?;
    Ok(())
}

/// 绑定 127.0.0.1:port；port=0 时由内核分配临时端口。
pub fn bind_localhost(port: u16) -> Result<TcpListener, String> {
    let addr: SocketAddr = format!("{BIND_HOST}:{port}")
        .parse()
        .map_err(|e| format!("地址无效：{e}"))?;
    let listener = TcpListener::bind(addr).map_err(|e| {
        format!("无法绑定 {BIND_HOST}:{port}：{e}。请换 --port 或确认未被占用。")
    })?;
    let local = listener
        .local_addr()
        .map_err(|e| format!("读取绑定地址失败：{e}"))?;
    if !local.ip().is_loopback() {
        return Err(format!(
            "拒绝非 loopback 绑定：{}（只允许 127.0.0.1）",
            local.ip()
        ));
    }
    // 接受连接时不要永久阻塞在 accept（配合 shutdown）。
    listener
        .set_nonblocking(true)
        .map_err(|e| format!("set_nonblocking 失败：{e}"))?;
    Ok(listener)
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServeJobView {
    pub id: u64,
    pub route: String,
    pub body: String,
}

struct ServeJob {
    id: u64,
    route: ServeRoute,
    body: String,
    reply_tx: SyncSender<HttpReply>,
}

#[derive(Debug, Clone)]
struct HttpReply {
    status: u16,
    body: String,
    content_type: String,
}

struct ServeInner {
    token: String,
    #[allow(dead_code)]
    port: u16,
    shutdown: Arc<AtomicBool>,
    /// HTTP → 前端：有界队列（容量 1 也行；并发会在 HTTP 线程排队等 send）
    job_tx: SyncSender<ServeJob>,
    job_rx: Mutex<Receiver<ServeJob>>,
    /// 前端持有的「当前正在处理」的 reply 通道（按 id）
    pending_replies: Mutex<HashMap<u64, SyncSender<HttpReply>>>,
    next_id: AtomicU64,
    listener_thread: Mutex<Option<thread::JoinHandle<()>>>,
}

static SERVE: Mutex<Option<Arc<ServeInner>>> = Mutex::new(None);

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServeStartInfo {
    pub host: String,
    pub port: u16,
    pub token: String,
    pub base_url: String,
}

pub fn serve_start(port: u16) -> Result<ServeStartInfo, String> {
    let mut guard = SERVE.lock().map_err(|_| "serve 状态锁损坏".to_string())?;
    if guard.is_some() {
        return Err("serve 已在运行".into());
    }

    let token = generate_token()?;
    let listener = bind_localhost(port)?;
    let bound = listener
        .local_addr()
        .map_err(|e| format!("读取绑定地址失败：{e}"))?;
    let bound_port = bound.port();

    let (job_tx, job_rx) = mpsc::sync_channel::<ServeJob>(8);
    let shutdown = Arc::new(AtomicBool::new(false));
    let inner = Arc::new(ServeInner {
        token: token.clone(),
        port: bound_port,
        shutdown: shutdown.clone(),
        job_tx: job_tx.clone(),
        job_rx: Mutex::new(job_rx),
        pending_replies: Mutex::new(HashMap::new()),
        next_id: AtomicU64::new(1),
        listener_thread: Mutex::new(None),
    });

    let thread_inner = Arc::clone(&inner);
    let handle = thread::Builder::new()
        .name("cli-serve-http".into())
        .spawn(move || accept_loop(listener, thread_inner))
        .map_err(|e| format!("启动 serve 监听线程失败：{e}"))?;

    *inner
        .listener_thread
        .lock()
        .map_err(|_| "listener_thread 锁损坏".to_string())? = Some(handle);

    *guard = Some(Arc::clone(&inner));

    Ok(ServeStartInfo {
        host: BIND_HOST.to_string(),
        port: bound_port,
        token,
        base_url: format!("http://{BIND_HOST}:{bound_port}"),
    })
}

pub fn serve_poll(timeout_ms: u64) -> Result<Option<ServeJobView>, String> {
    let inner = {
        let guard = SERVE.lock().map_err(|_| "serve 状态锁损坏".to_string())?;
        guard
            .as_ref()
            .cloned()
            .ok_or_else(|| "serve 未启动".to_string())?
    };
    if inner.shutdown.load(Ordering::SeqCst) {
        return Ok(None);
    }
    let rx = inner
        .job_rx
        .lock()
        .map_err(|_| "job_rx 锁损坏".to_string())?;
    match rx.recv_timeout(Duration::from_millis(timeout_ms.max(1))) {
        Ok(job) => {
            let view = ServeJobView {
                id: job.id,
                route: job.route.as_str().to_string(),
                body: job.body.clone(),
            };
            inner
                .pending_replies
                .lock()
                .map_err(|_| "pending_replies 锁损坏".to_string())?
                .insert(job.id, job.reply_tx);
            Ok(Some(view))
        }
        Err(RecvTimeoutError::Timeout) => Ok(None),
        Err(RecvTimeoutError::Disconnected) => Ok(None),
    }
}

pub fn serve_respond(id: u64, status: u16, body: String) -> Result<(), String> {
    let inner = {
        let guard = SERVE.lock().map_err(|_| "serve 状态锁损坏".to_string())?;
        guard
            .as_ref()
            .cloned()
            .ok_or_else(|| "serve 未启动".to_string())?
    };
    let tx = inner
        .pending_replies
        .lock()
        .map_err(|_| "pending_replies 锁损坏".to_string())?
        .remove(&id)
        .ok_or_else(|| format!("没有 id={id} 的待回复请求（可能已超时）"))?;
    tx.send(HttpReply {
        status,
        body,
        content_type: "application/json; charset=utf-8".into(),
    })
    .map_err(|_| "HTTP 侧已断开（客户端可能已超时关闭）".to_string())?;
    Ok(())
}

pub fn serve_stop() -> Result<(), String> {
    let inner = {
        let mut guard = SERVE.lock().map_err(|_| "serve 状态锁损坏".to_string())?;
        guard.take()
    };
    let Some(inner) = inner else {
        return Ok(());
    };
    inner.shutdown.store(true, Ordering::SeqCst);
    // 唤醒可能卡在 send 上的 HTTP 线程：关掉 job 通道。
    // SyncSender 在 drop 全部 sender 后 Receiver 会断开；这里保留 inner.job_tx，
    // 靠 accept_loop 看到 shutdown 退出。
    if let Ok(mut t) = inner.listener_thread.lock() {
        if let Some(handle) = t.take() {
            let _ = handle.join();
        }
    }
    // 未回复的请求：尽量通知失败
    if let Ok(mut map) = inner.pending_replies.lock() {
        for (_, tx) in map.drain() {
            let _ = tx.send(HttpReply {
                status: 503,
                body: r#"{"ok":false,"error":{"code":"server_stopping","message":"serve 正在关闭","hint":"serve 正在退出，稍后重新启动 clash-assist --cli serve 再请求。"}}"#.into(),
                content_type: "application/json; charset=utf-8".into(),
            });
        }
    }
    Ok(())
}

#[allow(dead_code)]
pub fn serve_is_running() -> bool {
    SERVE
        .lock()
        .ok()
        .and_then(|g| g.as_ref().map(|i| !i.shutdown.load(Ordering::SeqCst)))
        .unwrap_or(false)
}

fn accept_loop(listener: TcpListener, inner: Arc<ServeInner>) {
    while !inner.shutdown.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, _)) => {
                let inner2 = Arc::clone(&inner);
                // 每个连接一个短线程；WebView 任务通过有界 channel 串行化。
                let _ = thread::Builder::new()
                    .name("cli-serve-conn".into())
                    .spawn(move || {
                        if let Err(e) = handle_connection(stream, &inner2) {
                            eprintln!("cli serve: 处理连接失败：{e}");
                        }
                    });
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(50));
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => {
                if !inner.shutdown.load(Ordering::SeqCst) {
                    eprintln!("cli serve: accept 失败：{e}");
                }
                thread::sleep(Duration::from_millis(100));
            }
        }
    }
}

fn handle_connection(mut stream: TcpStream, inner: &ServeInner) -> Result<(), String> {
    // macOS/BSD 上 accept 出的连接继承监听 socket 的 O_NONBLOCK（accept_loop 需要
    // 监听非阻塞来轮询 shutdown）。不清掉的话，下面的读写超时形同虚设：请求字节
    // 未到时的首次 read 立即 EAGAIN，连接被误丢（CI 实测 os error 35）。
    stream
        .set_nonblocking(false)
        .map_err(|e| format!("clear inherited nonblocking: {e}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .ok();
    stream
        .set_write_timeout(Some(Duration::from_secs(30)))
        .ok();

    let req = read_http_request(&mut stream)?;
    if inner.shutdown.load(Ordering::SeqCst) {
        write_response(
            &mut stream,
            503,
            "application/json; charset=utf-8",
            r#"{"ok":false,"error":{"code":"server_stopping","message":"serve 正在关闭","hint":"serve 正在退出，稍后重新启动 clash-assist --cli serve 再请求。"}}"#,
        )?;
        return Ok(());
    }

    if !token_matches(&req.headers, &inner.token) {
        write_response(
            &mut stream,
            401,
            "application/json; charset=utf-8",
            r#"{"ok":false,"error":{"code":"unauthorized","message":"缺少或错误的 token。请用 Authorization: Bearer <token> 或 X-Egress-Token: <token>。","hint":"token 在 serve 启动时打印（stdout 横幅 data.token），每次启动都会变；请用最新 token。"}}"#,
        )?;
        return Ok(());
    }

    let Some(route) = match_route(&req.method, &req.path) else {
        // 方法不对但路径像已知？给 405；否则 404
        let status = if looks_like_known_path(&req.path) {
            405
        } else {
            404
        };
        let msg = if status == 405 {
            r#"{"ok":false,"error":{"code":"method_not_allowed","message":"方法不允许。可用：GET /health；POST /v1/discover|nodes|gate|env|check/current|check/all|check/node","hint":"业务路由都用 POST；健康检查用 GET /health。"}}"#
        } else {
            r#"{"ok":false,"error":{"code":"not_found","message":"未知路径。可用：GET /health；POST /v1/discover|nodes|gate|env|check/current|check/all|check/node","hint":"业务路由都用 POST；健康检查用 GET /health。"}}"#
        };
        write_response(&mut stream, status, "application/json; charset=utf-8", msg)?;
        return Ok(());
    };

    if route == ServeRoute::Health {
        write_response(
            &mut stream,
            200,
            "application/json; charset=utf-8",
            r#"{"ok":true}"#,
        )?;
        return Ok(());
    }

    let id = inner.next_id.fetch_add(1, Ordering::SeqCst);
    let (reply_tx, reply_rx) = mpsc::sync_channel::<HttpReply>(1);
    let job = ServeJob {
        id,
        route,
        body: req.body,
        reply_tx,
    };

    // 排队交给前端；若 channel 满则阻塞（串行化并发）。
    if inner.job_tx.send(job).is_err() {
        write_response(
            &mut stream,
            503,
            "application/json; charset=utf-8",
            r#"{"ok":false,"error":{"code":"server_stopping","message":"serve 已关闭","hint":"重新启动 clash-assist --cli serve，并使用新打印的 token。"}}"#,
        )?;
        return Ok(());
    }

    match reply_rx.recv_timeout(Duration::from_secs(JOB_WAIT_SECS)) {
        Ok(reply) => {
            write_response(&mut stream, reply.status, &reply.content_type, &reply.body)?;
        }
        Err(_) => {
            write_response(
                &mut stream,
                504,
                "application/json; charset=utf-8",
                r#"{"ok":false,"error":{"code":"timeout","message":"等待 WebView 处理超时","hint":"check all 节点多时会很久：用 --limit / --region 缩小范围，或改用一次性 CLI。"}}"#,
            )?;
        }
    }
    Ok(())
}

fn looks_like_known_path(path: &str) -> bool {
    let path = path.split('?').next().unwrap_or(path);
    matches!(
        path,
        "/health"
            | "/v1/discover"
            | "/v1/nodes"
            | "/v1/gate"
            | "/v1/env"
            | "/v1/check/current"
            | "/v1/check/all"
            | "/v1/check/node"
    )
}

struct RawRequest {
    method: String,
    path: String,
    headers: HashMap<String, String>,
    body: String,
}

fn read_http_request(stream: &mut TcpStream) -> Result<RawRequest, String> {
    let mut buf = Vec::with_capacity(4096);
    let mut tmp = [0u8; 2048];
    let header_end;
    loop {
        let n = stream
            .read(&mut tmp)
            .map_err(|e| format!("读请求失败：{e}"))?;
        if n == 0 {
            return Err("客户端关闭连接".into());
        }
        buf.extend_from_slice(&tmp[..n]);
        if buf.len() > MAX_HEADER_BYTES {
            return Err("请求头过大".into());
        }
        if let Some(pos) = find_header_end(&buf) {
            header_end = pos;
            break;
        }
    }

    let header_bytes = &buf[..header_end];
    let header_str = String::from_utf8_lossy(header_bytes);
    let mut lines = header_str.split("\r\n");
    let request_line = lines.next().ok_or("空请求行")?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().ok_or("缺方法")?.to_string();
    let path = parts.next().ok_or("缺路径")?.to_string();

    let mut headers = HashMap::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        if let Some((k, v)) = line.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }

    let content_length: usize = headers
        .get("content-length")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    if content_length > MAX_BODY_BYTES {
        return Err("请求体过大".into());
    }

    let mut body_buf = buf[header_end + 4..].to_vec(); // skip \r\n\r\n
    while body_buf.len() < content_length {
        let n = stream
            .read(&mut tmp)
            .map_err(|e| format!("读 body 失败：{e}"))?;
        if n == 0 {
            break;
        }
        body_buf.extend_from_slice(&tmp[..n]);
        if body_buf.len() > MAX_BODY_BYTES {
            return Err("请求体过大".into());
        }
    }
    body_buf.truncate(content_length);
    let body = String::from_utf8_lossy(&body_buf).to_string();

    Ok(RawRequest {
        method,
        path,
        headers,
        body,
    })
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

fn write_response(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &str,
) -> Result<(), String> {
    let reason = match status {
        200 => "OK",
        401 => "Unauthorized",
        404 => "Not Found",
        405 => "Method Not Allowed",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        _ => "Error",
    };
    let bytes = body.as_bytes();
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        bytes.len()
    );
    stream
        .write_all(head.as_bytes())
        .map_err(|e| format!("写响应头失败：{e}"))?;
    stream
        .write_all(bytes)
        .map_err(|e| format!("写响应体失败：{e}"))?;
    stream.flush().map_err(|e| format!("flush 失败：{e}"))?;
    Ok(())
}

/// 人类可读就绪提示（打到 stderr）。
#[allow(dead_code)]
pub fn format_ready_human(info: &ServeStartInfo) -> String {
    format!(
        "serve 已就绪：{base}  （只监听 127.0.0.1）\n\
         token: {token}\n\
         每个请求需带：Authorization: Bearer {token}\n\
         或：X-Egress-Token: {token}\n\
         路由：GET /health；POST /v1/discover|nodes|gate|env|check/current|check/all|check/node\n\
         Ctrl+C 停止监听并退出。",
        base = info.base_url,
        token = info.token,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpStream as StdTcpStream;
    use std::sync::Mutex;

    /// 全局 SERVE 单例：live 测试必须串行。
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn match_route_known() {
        assert_eq!(match_route("GET", "/health"), Some(ServeRoute::Health));
        assert_eq!(
            match_route("POST", "/v1/discover"),
            Some(ServeRoute::Discover)
        );
        assert_eq!(
            match_route("POST", "/v1/check/node?x=1"),
            Some(ServeRoute::CheckNode)
        );
        assert_eq!(match_route("POST", "/v1/nodes"), Some(ServeRoute::Nodes));
        assert_eq!(match_route("GET", "/v1/discover"), None);
        assert_eq!(match_route("POST", "/v1/nope"), None);
        assert_eq!(match_route("PUT", "/health"), None);
    }

    #[test]
    fn auth_bearer_and_header() {
        let mut h = HashMap::new();
        assert!(!token_matches(&h, "secret"));
        h.insert("authorization".into(), "Bearer secret".into());
        assert!(token_matches(&h, "secret"));
        assert!(!token_matches(&h, "other"));
        h.clear();
        h.insert("x-egress-token".into(), "tok".into());
        assert!(token_matches(&h, "tok"));
    }

    #[test]
    fn bind_is_loopback_only() {
        let listener = bind_localhost(0).expect("bind");
        let addr = listener.local_addr().unwrap();
        assert!(addr.ip().is_loopback(), "{addr}");
        assert_eq!(format!("{}", addr.ip()), "127.0.0.1");
        assert_ne!(addr.port(), 0);
    }

    #[test]
    fn generate_token_length() {
        let t = generate_token().unwrap();
        assert_eq!(t.len(), 48);
        assert!(t.chars().all(|c| c.is_ascii_hexdigit()));
    }

    fn http_exchange(addr: &str, raw: &str) -> (u16, String) {
        let mut stream = StdTcpStream::connect(addr).expect("connect");
        stream.set_read_timeout(Some(Duration::from_secs(5))).ok();
        stream.write_all(raw.as_bytes()).unwrap();
        let mut buf = Vec::new();
        let mut tmp = [0u8; 4096];
        loop {
            match stream.read(&mut tmp) {
                Ok(0) => break,
                Ok(n) => buf.extend_from_slice(&tmp[..n]),
                Err(_) => break,
            }
        }
        let text = String::from_utf8_lossy(&buf).to_string();
        let status = text
            .lines()
            .next()
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let body = text
            .split("\r\n\r\n")
            .nth(1)
            .unwrap_or("")
            .to_string();
        (status, body)
    }

    #[test]
    fn live_auth_reject_and_health() {
        let _guard = TEST_LOCK.lock().unwrap();
        let _ = serve_stop();
        let info = serve_start(0).expect("start");
        let addr = format!("127.0.0.1:{}", info.port);

        let (st, body) = http_exchange(
            &addr,
            "GET /health HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n",
        );
        assert_eq!(st, 401, "{body}");
        assert!(body.contains("unauthorized"));

        let (st, body) = http_exchange(
            &addr,
            &format!(
                "GET /health HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {}\r\nConnection: close\r\n\r\n",
                info.token
            ),
        );
        assert_eq!(st, 200, "{body}");
        assert_eq!(body.trim(), r#"{"ok":true}"#);

        let (st, body) = http_exchange(
            &addr,
            &format!(
                "GET /nope HTTP/1.1\r\nHost: 127.0.0.1\r\nX-Egress-Token: {}\r\nConnection: close\r\n\r\n",
                info.token
            ),
        );
        assert_eq!(st, 404, "{body}");

        let (st, body) = http_exchange(
            &addr,
            &format!(
                "POST /health HTTP/1.1\r\nHost: 127.0.0.1\r\nX-Egress-Token: {}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                info.token
            ),
        );
        assert_eq!(st, 405, "{body}");

        serve_stop().unwrap();
    }

    /// 锁定 0.2.8 发布前 CI 抓到的生产缺陷：serve 的监听 socket 是非阻塞的
    /// （accept_loop 靠它轮询 shutdown），而 macOS/BSD 上 accept 出的连接会
    /// **继承** O_NONBLOCK——`handle_connection` 虽设了 30s 读超时，非阻塞 fd 下
    /// 超时并不生效，请求字节尚未到达的首次 read 立即返回 EAGAIN（os error 35），
    /// 连接被当作「读请求失败」丢弃，客户端拿到空响应（状态 0）。
    /// 本测试用「握手后延迟 250ms 再发」的客户端确定性踩中该窗口：修复前必挂。
    #[test]
    fn live_request_after_connect_delay_is_answered() {
        let _guard = TEST_LOCK.lock().unwrap();
        let _ = serve_stop();
        let info = serve_start(0).expect("start");
        let addr = format!("127.0.0.1:{}", info.port);

        let mut client = StdTcpStream::connect(&addr).expect("connect");
        std::thread::sleep(Duration::from_millis(250));
        client
            .write_all(
                b"GET /health HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n",
            )
            .expect("write");
        let mut buf = Vec::new();
        client.read_to_end(&mut buf).expect("read");
        let text = String::from_utf8_lossy(&buf).to_string();
        let status: u16 = text
            .lines()
            .next()
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        assert_eq!(
            status, 401,
            "延迟到达的请求也必须被应答，而不是被 EAGAIN 丢弃：{text:?}"
        );

        serve_stop().unwrap();
    }

    #[test]
    fn live_post_forwards_to_poll() {
        let _guard = TEST_LOCK.lock().unwrap();
        let _ = serve_stop();
        let info = serve_start(0).expect("start");
        let addr = format!("127.0.0.1:{}", info.port);
        let token = info.token.clone();

        let client = thread::spawn(move || {
            http_exchange(
                &addr,
                &format!(
                    "POST /v1/discover HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {token}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                ),
            )
        });

        // 前端侧取走任务并回复
        let mut job = None;
        for _ in 0..50 {
            if let Some(j) = serve_poll(100).unwrap() {
                job = Some(j);
                break;
            }
        }
        let job = job.expect("should receive job");
        assert_eq!(job.route, "discover");
        serve_respond(
            job.id,
            200,
            r#"{"ok":true,"version":"0.1.16","command":"discover","ranAt":"t","data":{}}"#.into(),
        )
        .unwrap();

        let (st, body) = client.join().unwrap();
        assert_eq!(st, 200, "{body}");
        assert!(body.contains(r#""command":"discover""#));

        serve_stop().unwrap();
    }
}
