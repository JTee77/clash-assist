//! HTTP transport to the Mihomo controller: TCP (via `reqwest`) and, on
//! macOS/Linux, a raw Unix-domain socket. Also the plain "fetch through the
//! mixed-port proxy" used by reachability checks. Raw response/chunked decoding
//! is delegated to `super::http_parse`; bandwidth timing lives in `super::timed`.

use std::time::Duration;

#[cfg(unix)]
use super::http_parse::parse_http_response_bytes;
use super::types::{UnixHttpResult, MAX_BODY_BYTES};

/// 边读边计数：累计超过 `MAX_BODY_BYTES` 立即中止（实现见 `crate::http_body`）。
/// `Content-Length` 已声明超限时直接拒绝、不读 body。
async fn read_body_capped(res: reqwest::Response) -> Result<(u16, String), String> {
    read_body_capped_with(res, MAX_BODY_BYTES).await
}

async fn read_body_capped_with(
    res: reqwest::Response,
    max: usize,
) -> Result<(u16, String), String> {
    let status = res.status().as_u16();
    let buf = crate::http_body::read_body_capped(res, max).await.map_err(|e| {
        if e.contains("too large") {
            format!("{e} — refusing to ship to webview")
        } else {
            e
        }
    })?;
    let body = String::from_utf8_lossy(&buf).into_owned();
    Ok((status, body))
}

/// Windows 上控制器 TCP 通道被关闭时返回的错误（M0 fail-closed）。
#[cfg_attr(not(windows), allow(dead_code))]
pub const WINDOWS_TCP_CONTROLLER_UNAVAILABLE: &str =
    "Windows 版控制器连接尚未启用（缺少监听者校验，M1）";

/// Async HTTP to Mihomo controller over TCP (preferred path from Tauri commands).
///
/// 这是控制器 TCP 请求的唯一出口：`mihomo_http` 命令、`list_nodes_async` 的 TCP
/// 路径（含 Unix 套接字失败后的回落）和 `/providers/proxies` 请求都经过这里。
///
/// Windows：一律不建立连接、不发送任何字节（更不会带 `Authorization: Bearer`），
/// 直接返回 [`WINDOWS_TCP_CONTROLLER_UNAVAILABLE`]。方案 §2.1 规则 4（M-3 / C-1 / C-4）
/// 要求只有 `listener-verified` 才能发送 secret，而 M0 还没有监听者校验。
pub async fn http_via_tcp_async(
    host: &str,
    port: u16,
    method: &str,
    path: &str,
    body: Option<&str>,
    secret: &str,
    timeout_ms: u64,
) -> Result<UnixHttpResult, String> {
    #[cfg(windows)]
    {
        // TODO(M1) verify_tcp_listener: 换成 `platform::verify_tcp_listener(addr)` 判定，
        // 只有 listener-verified 才发送 Bearer；listener-unverified 不发 secret（§2.1 规则 4、C-4）。
        let _ = (host, port, method, path, body, secret, timeout_ms);
        Err(WINDOWS_TCP_CONTROLLER_UNAVAILABLE.to_string())
    }
    #[cfg(not(windows))]
    {
        http_via_tcp_inner(host, port, method, path, body, secret, timeout_ms).await
    }
}

#[cfg(not(windows))]
async fn http_via_tcp_inner(
    host: &str,
    port: u16,
    method: &str,
    path: &str,
    body: Option<&str>,
    secret: &str,
    timeout_ms: u64,
) -> Result<UnixHttpResult, String> {
    let url = format!("http://{host}:{port}{path}");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(timeout_ms))
        .no_proxy()
        .build()
        .map_err(|e| format!("client: {e}"))?;

    let mut builder = match method.to_uppercase().as_str() {
        "GET" => client.get(&url),
        "PUT" => client.put(&url),
        "POST" => client.post(&url),
        "DELETE" => client.delete(&url),
        other => return Err(format!("unsupported method: {other}")),
    };

    builder = builder.header("Content-Type", "application/json");
    if !secret.is_empty() {
        builder = builder.header("Authorization", format!("Bearer {secret}"));
    }
    if let Some(b) = body {
        builder = builder.body(b.to_string());
    }

    let res = builder.send().await.map_err(|e| format!("request: {e}"))?;
    let (status, body) = read_body_capped(res).await?;
    Ok(UnixHttpResult { status, body })
}

/// HTTP over a Unix domain socket — the macOS/Linux local-IPC transport.
#[cfg(unix)]
pub fn http_via_unix(
    method: &str,
    path: &str,
    body: Option<&str>,
    secret: &str,
    sock_path: Option<&str>,
    timeout_ms: u64,
) -> Result<UnixHttpResult, String> {
    use std::io::{Read, Write};
    use std::os::unix::net::UnixStream;

    let sock = sock_path.ok_or_else(|| {
        "unix socket path not provided (refusing Verge default for non-Verge clients)".to_string()
    })?;
    #[allow(clippy::disallowed_methods, reason = "仅 Unix，不在 S-R1 的 Windows 风险面")]
    let sock_exists = std::path::Path::new(sock).exists();
    if !sock_exists {
        return Err(format!("unix socket not found: {sock}"));
    }
    // 请求行是手写拼接的：方法 / 路径带 CR、LF 或空格会注入额外请求头。
    super::guard::check_controller_method(method)?;
    if !path.starts_with('/') || !path.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(format!("invalid request path: {path:?}"));
    }
    // 0.2.9：拒绝符号链接路径；连接后用对端凭证（peer uid）判定可信，避免 path TOCTOU。
    crate::platform::ensure_sock_path_trusted(sock)?;

    let timeout = Duration::from_millis(timeout_ms);
    let mut stream = UnixStream::connect(sock).map_err(|e| format!("connect {sock}: {e}"))?;
    crate::platform::ensure_connected_peer_trusted(&stream, sock)?;
    let _ = stream.set_read_timeout(Some(timeout));
    let _ = stream.set_write_timeout(Some(timeout));

    let body_bytes = body.unwrap_or("").as_bytes();
    let auth = if secret.is_empty() {
        String::new()
    } else {
        format!("Authorization: Bearer {secret}\r\n")
    };

    // Accept-Encoding: identity — raw UnixStream does not auto-decode gzip like curl.
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: localhost\r\n{auth}Content-Type: application/json\r\nAccept-Encoding: identity\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body_bytes.len()
    );

    stream
        .write_all(req.as_bytes())
        .map_err(|e| format!("write headers: {e}"))?;
    if !body_bytes.is_empty() {
        stream
            .write_all(body_bytes)
            .map_err(|e| format!("write body: {e}"))?;
    }

    // 最多读 MAX_BODY_BYTES + 1 字节：多出的那 1 字节用来判定超限，不会无界增长内存。
    let mut raw = Vec::new();
    (&mut stream)
        .take(MAX_BODY_BYTES as u64 + 1)
        .read_to_end(&mut raw)
        .map_err(|e| format!("read: {e}"))?;

    if raw.len() > MAX_BODY_BYTES {
        return Err(format!(
            "unix response too large (> {} bytes max)",
            MAX_BODY_BYTES
        ));
    }

    parse_http_response_bytes(&raw)
}

/// Non-unix targets (e.g. Windows): no Unix-domain sockets. Local IPC on Windows
/// will be the verified named-pipe transport (M1, `pipe::open_verified`); until then
/// there is **no** working controller transport on Windows — the TCP path
/// (`http_via_tcp_async`) is also closed there (fail-closed until M1's
/// `verify_tcp_listener`, plan §2.1 rule 4). This stub does no I/O.
#[cfg(not(unix))]
pub fn http_via_unix(
    _method: &str,
    _path: &str,
    _body: Option<&str>,
    _secret: &str,
    _sock_path: Option<&str>,
    _timeout_ms: u64,
) -> Result<UnixHttpResult, String> {
    Err(
        "unix socket transport unavailable on this platform".into(),
    )
}

/// Async fetch a URL optionally via local mixed-port HTTP proxy.
pub async fn proxy_fetch_async(
    url: &str,
    mixed_port: Option<u16>,
    user_agent: Option<&str>,
    timeout_ms: u64,
    cancel: Option<super::cancel::CancelWait>,
) -> Result<UnixHttpResult, String> {
    let work = proxy_fetch_inner(url, mixed_port, user_agent, timeout_ms);
    if let Some(cancel) = cancel {
        return match futures_util::future::select(std::pin::pin!(work), std::pin::pin!(cancel)).await {
            futures_util::future::Either::Left((result, _)) => result,
            futures_util::future::Either::Right((_, _)) => Err("已取消".into()),
        };
    }
    work.await
}

async fn proxy_fetch_inner(
    url: &str,
    mixed_port: Option<u16>,
    user_agent: Option<&str>,
    timeout_ms: u64,
) -> Result<UnixHttpResult, String> {
    let mut builder = reqwest::Client::builder()
        .timeout(Duration::from_millis(timeout_ms))
        .redirect(super::guard::probe_redirect_policy(10))
        .danger_accept_invalid_certs(false);

    if let Some(port) = mixed_port {
        let proxy_url = format!("http://127.0.0.1:{port}");
        let proxy = reqwest::Proxy::all(&proxy_url).map_err(|e| format!("proxy: {e}"))?;
        builder = builder.proxy(proxy);
    } else {
        builder = builder.no_proxy();
    }

    let client = builder.build().map_err(|e| format!("client: {e}"))?;
    let mut req = client.get(url);
    if let Some(ua) = user_agent {
        req = req.header("User-Agent", ua);
    }
    let res = req.send().await.map_err(|e| format!("request: {e}"))?;
    let (status, body) = read_body_capped(res).await?;
    Ok(UnixHttpResult { status, body })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 本地夹具：接一个连接，读掉请求头后原样写 `response`，然后关闭。
    fn serve_once(response: Vec<u8>) -> (u16, std::thread::JoinHandle<()>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let h = std::thread::spawn(move || {
            if let Ok((mut s, _)) = listener.accept() {
                let _ = s.set_nonblocking(false);
                let _ = s.set_read_timeout(Some(std::time::Duration::from_millis(1000)));
                let mut buf = [0u8; 4096];
                let mut got = Vec::new();
                while !got.windows(4).any(|w| w == b"\r\n\r\n") {
                    match s.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => got.extend_from_slice(&buf[..n]),
                    }
                }
                let _ = s.write_all(&response);
            }
        });
        (port, h)
    }

    fn fetch_capped(port: u16, max: usize) -> Result<(u16, String), String> {
        tauri::async_runtime::block_on(async move {
            let client = reqwest::Client::builder().no_proxy().build().unwrap();
            let res = client
                .get(format!("http://127.0.0.1:{port}/"))
                .send()
                .await
                .map_err(|e| e.to_string())?;
            read_body_capped_with(res, max).await
        })
    }

    #[test]
    fn body_cap_enforced_while_streaming_chunked() {
        // 无 Content-Length 的 chunked 响应，总量 4 KiB > 1 KiB 上限：必须在读流过程中拒绝。
        let mut resp = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n".to_vec();
        for _ in 0..8 {
            resp.extend_from_slice(b"200\r\n");
            resp.extend_from_slice(&[b'a'; 0x200]);
            resp.extend_from_slice(b"\r\n");
        }
        resp.extend_from_slice(b"0\r\n\r\n");
        let (port, h) = serve_once(resp);
        let r = fetch_capped(port, 1024);
        let _ = h.join();
        let err = r.expect_err("oversized chunked body must be rejected");
        assert!(err.contains("too large"), "{err}");
    }

    #[test]
    fn body_cap_rejects_declared_content_length() {
        let resp = b"HTTP/1.1 200 OK\r\nContent-Length: 999999\r\nConnection: close\r\n\r\nabc".to_vec();
        let (port, h) = serve_once(resp);
        let r = fetch_capped(port, 1024);
        let _ = h.join();
        assert!(r.expect_err("declared oversize must be rejected").contains("too large"));
    }

    #[test]
    fn body_under_cap_is_returned() {
        let resp = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello".to_vec();
        let (port, h) = serve_once(resp);
        let r = fetch_capped(port, 1024);
        let _ = h.join();
        assert_eq!(r.unwrap(), (200, "hello".to_string()));
    }

    #[test]
    fn smoke_http_via_tcp_invalid_port_no_panic() {
        let result = std::panic::catch_unwind(|| {
            tauri::async_runtime::block_on(http_via_tcp_async(
                "127.0.0.1",
                1,
                "GET",
                "/",
                None,
                "",
                500,
            ))
        });
        assert!(result.is_ok(), "http_via_tcp_async panicked");
        let inner = result.unwrap();
        assert!(inner.is_err(), "expected Err to closed port, got {inner:?}");
    }

    /// 锁住控制器 TCP 路径的超时语义：对面收下连接但不回任何字节时，调用必须被
    /// `timeout_ms` 掐断，而不是挂到对面主动关连接为止。
    ///
    /// 判据用**耗时**而不是错误字符串：`http_via_tcp_async` 把 `reqwest::Error`
    /// 映射成了 String，而 reqwest 对总超时的 Display 是
    /// `error sending request for url (...)`，`is_timeout()` 只能从 source 链里
    /// 找到 `TimedOut`——字符串层面区分不出超时与发送失败。对面撑 1500 ms 才关连接，
    /// 所以「明显早于 1500 ms 结束」只可能是 250 ms 预算生效；下限则排除了连接被
    /// 立刻重置这种假通过。
    ///
    /// 这条测试是 0.2.7 性能核查顺手补的：曾考虑改成共享 Client，实测后回退，
    /// 但「超时到底有没有生效」原本没有任何测试覆盖。
    // Windows 上控制器 TCP 通道是 fail-closed 的（不建连），超时语义不适用；
    // 对应的 Windows 断言见 `windows_tcp_fail_closed_tests`。
    #[cfg(not(windows))]
    #[test]
    fn http_via_tcp_times_out_on_silent_server() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let worker = std::thread::spawn(move || {
            // 收下连接后什么都不回，靠 sleep 撑住 socket（早于超时返回会误判成 reset）
            if let Ok((stream, _)) = listener.accept() {
                std::thread::sleep(std::time::Duration::from_millis(1500));
                drop(stream);
            }
        });

        let started = std::time::Instant::now();
        let result =
            tauri::async_runtime::block_on(http_via_tcp_async("127.0.0.1", port, "GET", "/proxies", None, "", 250));
        let elapsed = started.elapsed();

        let err = match result {
            Ok(ok) => {
                worker.join().unwrap();
                panic!("沉默的服务端不该返回成功：{ok:?}");
            }
            Err(e) => e,
        };
        assert!(
            elapsed >= std::time::Duration::from_millis(200),
            "只等了 {elapsed:?}（预算 250 ms），说明不是超时结束的：{err}"
        );
        assert!(
            elapsed < std::time::Duration::from_millis(1200),
            "250 ms 预算却等了 {elapsed:?}，说明超时没生效"
        );
        worker.join().unwrap();
    }

    #[test]
    fn smoke_proxy_fetch_invalid_port_no_panic() {
        let result = std::panic::catch_unwind(|| {
            tauri::async_runtime::block_on(proxy_fetch_async(
                "https://www.gstatic.com/generate_204",
                Some(1),
                None,
                800,
                None,
            ))
        });
        assert!(result.is_ok(), "proxy_fetch_async panicked");
        let inner = result.unwrap();
        assert!(inner.is_err(), "expected Err via dead proxy, got {inner:?}");
    }

    #[test]
    fn smoke_proxy_fetch_no_proxy_client_builds() {
        // Ensures Client::builder + no_proxy path does not panic.
        // May Err on network (sandbox / offline) — that is fine; must not panic.
        let result = std::panic::catch_unwind(|| {
            tauri::async_runtime::block_on(proxy_fetch_async(
                "https://www.gstatic.com/generate_204",
                None,
                None,
                3000,
                None,
            ))
        });
        assert!(result.is_ok(), "proxy_fetch_async (no_proxy) panicked");
        // Ok or Err both acceptable; panic is not.
        let _ = result.unwrap();
    }
}

/// P-1（PR #45 评审）：Windows 上控制器 TCP 出口必须 fail-closed——不建连、不发 secret。
/// 判据：本地监听端口上**从未出现过**待 accept 的连接（连接即使随后被关闭，也会留在
/// backlog 里被 accept 到，所以“accept 一直 WouldBlock”能证明从未尝试连接）。
#[cfg(all(test, windows))]
pub(crate) mod windows_tcp_fail_closed_tests {
    use super::*;
    use std::net::TcpListener;

    pub(crate) fn assert_never_accepted(listener: &TcpListener) {
        listener.set_nonblocking(true).unwrap();
        // 给可能存在的异步连接留出时间落入 backlog。
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
        while std::time::Instant::now() < deadline {
            match listener.accept() {
                Ok((_, peer)) => panic!("Windows 上不应向控制器建立 TCP 连接，却 accept 到了 {peer}"),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) => panic!("accept: {e}"),
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
    }

    #[test]
    fn http_via_tcp_never_connects_or_sends_secret_on_windows() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        for method in ["GET", "PUT", "POST", "DELETE"] {
            let r = tauri::async_runtime::block_on(http_via_tcp_async(
                "127.0.0.1",
                port,
                method,
                "/proxies",
                Some("{}"),
                "s3cret-must-not-leave",
                1000,
            ));
            assert_eq!(r.unwrap_err(), WINDOWS_TCP_CONTROLLER_UNAVAILABLE, "{method}");
        }
        assert_never_accepted(&listener);
    }
}
