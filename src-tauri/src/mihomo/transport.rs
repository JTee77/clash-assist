//! HTTP transport to the Mihomo controller: TCP (via `reqwest`) and, on
//! macOS/Linux, a raw Unix-domain socket. Also the plain "fetch through the
//! mixed-port proxy" used by reachability checks. Raw response/chunked decoding
//! is delegated to `super::http_parse`; bandwidth timing lives in `super::timed`.

use std::time::Duration;

use super::http_parse::parse_http_response_bytes;
use super::types::{UnixHttpResult, MAX_BODY_BYTES};

async fn read_body_capped(res: reqwest::Response) -> Result<(u16, String), String> {
    let status = res.status().as_u16();
    let bytes = res
        .bytes()
        .await
        .map_err(|e| format!("read body: {e}"))?;
    if bytes.len() > MAX_BODY_BYTES {
        return Err(format!(
            "response body too large ({} bytes > {} max) — refusing to ship to webview",
            bytes.len(),
            MAX_BODY_BYTES
        ));
    }
    let body = String::from_utf8_lossy(&bytes).into_owned();
    Ok((status, body))
}

/// Async HTTP to Mihomo controller over TCP (preferred path from Tauri commands).
pub async fn http_via_tcp_async(
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
    if !std::path::Path::new(sock).exists() {
        return Err(format!("unix socket not found: {sock}"));
    }

    let timeout = Duration::from_millis(timeout_ms);
    let mut stream = UnixStream::connect(sock).map_err(|e| format!("connect {sock}: {e}"))?;
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

    let mut raw = Vec::new();
    stream
        .read_to_end(&mut raw)
        .map_err(|e| format!("read: {e}"))?;

    if raw.len() > MAX_BODY_BYTES {
        return Err(format!(
            "unix response too large ({} bytes > {} max)",
            raw.len(),
            MAX_BODY_BYTES
        ));
    }

    parse_http_response_bytes(&raw)
}

/// Non-unix targets (e.g. Windows): local IPC would use a named pipe — not
/// implemented yet. TCP (`http_via_tcp_async`) remains the supported transport.
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
        "unix socket transport unavailable on this platform (use TCP external-controller)"
            .into(),
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
        .redirect(reqwest::redirect::Policy::limited(10))
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
