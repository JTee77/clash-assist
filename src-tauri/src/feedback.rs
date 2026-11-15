//! 发往反馈接口的单次 POST / GET。
//!
//! Apps Script 的 exec URL 会 302/303 到 `script.googleusercontent.com/macros/echo`。
//! 那次 POST 已经提交报告。echo 地址只允许 GET/HEAD，再 POST 同一份 JSON 会得到 405。
//! 这里用 `redirect::Policy::none()`，把状态码和 Location 交回前端；前端对 echo 改用 GET，不带正文。

use std::time::Duration;

use serde::Serialize;

const MAX_TEXT: usize = 256 * 1024;
const MAX_RESP: usize = 64 * 1024;

const FEEDBACK_HOSTS_EXEC: &str = "script.google.com";
const FEEDBACK_HOST_REDIRECT: &str = "script.googleusercontent.com";

#[derive(Debug, Serialize)]
pub struct FeedbackExchange {
    pub status: u16,
    pub location: Option<String>,
    pub body: String,
}

#[derive(Debug, Serialize)]
pub struct ReportHostInfo {
    pub os: &'static str,
    pub arch: &'static str,
}

pub fn host_info() -> ReportHostInfo {
    ReportHostInfo {
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
    }
}

/// 只允许官方 exec 地址，以及它跳过去的 `script.googleusercontent.com`。
/// GET 与 POST 用同一份名单，不新增主机，也不放行 exec 以外的 `script.google.com` 路径。
pub fn allow_feedback_url(raw: &str) -> Result<(), String> {
    let u = url::Url::parse(raw).map_err(|_| "url not allowed".to_string())?;
    // `Url::port()` 会把 https 的默认 443 吞掉，显式写上的 `:443` 也要拒绝。
    if u.scheme() != "https"
        || !u.username().is_empty()
        || u.password().is_some()
        || u.port().is_some()
        || explicit_port(raw)
    {
        return Err("url not allowed".into());
    }
    let host = u.host_str().unwrap_or("");
    if host == FEEDBACK_HOST_REDIRECT {
        return Ok(());
    }
    if host == FEEDBACK_HOSTS_EXEC
        && u.path().starts_with("/macros/s/")
        && u.path().ends_with("/exec")
        && u.query().is_none()
        && u.fragment().is_none()
    {
        return Ok(());
    }
    Err("url not allowed".into())
}

/// `https://host:443/...` 在解析后端口会被当成默认值。看原始字符串里的主机部分有没有冒号。
fn explicit_port(raw: &str) -> bool {
    let Some((_, rest)) = raw.split_once("://") else {
        return false;
    };
    let auth = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    let host = auth.rsplit('@').next().unwrap_or(auth);
    host.contains(':')
}

pub fn feedback_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        // 不记 cookie。echo 的 GET 不需要 cookie。
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(20))
        .user_agent(format!("clash-assist/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| format!("http client: {e}"))
}

async fn read_exchange(resp: reqwest::Response) -> Result<FeedbackExchange, String> {
    let status = resp.status().as_u16();
    let location = resp
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let bytes = crate::http_body::read_body_capped(resp, MAX_RESP).await?;
    Ok(FeedbackExchange {
        status,
        location,
        body: String::from_utf8_lossy(&bytes).into_owned(),
    })
}

/// 发一次 POST，不跟随重定向。正文原样送出。
pub async fn exchange_once(
    client: &reqwest::Client,
    url: &str,
    body: &str,
) -> Result<FeedbackExchange, String> {
    if body.len() > MAX_TEXT {
        return Err("report too large".into());
    }
    let resp = client
        .post(url)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(body.to_string())
        .send()
        .await
        .map_err(|e| format!("send: {e}"))?;
    read_exchange(resp).await
}

/// 发一次 GET，不跟随重定向，不带正文。
/// 用于读取 echo 上的 `{"ok":true}` / `{"ok":false}`。
pub async fn exchange_get(client: &reqwest::Client, url: &str) -> Result<FeedbackExchange, String> {
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("send: {e}"))?;
    read_exchange(resp).await
}

pub async fn post_feedback(url: &str, body: &str) -> Result<FeedbackExchange, String> {
    allow_feedback_url(url)?;
    let client = feedback_client()?;
    exchange_once(&client, url, body).await
}

pub async fn get_feedback(url: &str) -> Result<FeedbackExchange, String> {
    allow_feedback_url(url)?;
    let client = feedback_client()?;
    exchange_get(&client, url).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feedback_url_allowlist() {
        let ok = "https://script.google.com/macros/s/AKfycbypPAXzah4RI1px9Wybr9maQI0pJUqhIYuPFeNkJjmy1waiLS9rOrBudcfpysBAqs10rA/exec";
        assert!(allow_feedback_url(ok).is_ok());
        assert!(allow_feedback_url(
            "https://script.googleusercontent.com/macros/echo?user_content_key=abc&lib=xyz"
        )
        .is_ok());
        for bad in [
            "http://script.google.com/macros/s/abc/exec",
            "https://script.google.com/macros/s/abc/exec?x=1",
            "https://user:pw@script.google.com/macros/s/abc/exec",
            "https://script.google.com:443/macros/s/abc/exec",
            "https://evil.example/macros/s/abc/exec",
            "https://script.google.com/other",
            "https://script.google.com/macros/echo?x=1",
            "https://script.google.com.evil.example/macros/s/abc/exec",
            "https://evil.example/macros/echo",
        ] {
            assert!(allow_feedback_url(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn get_uses_the_same_allowlist() {
        let err = tauri::async_runtime::block_on(get_feedback(
            "https://evil.example/macros/echo?user_content_key=abc",
        ))
        .unwrap_err();
        assert!(err.contains("not allowed"), "{err}");
        let err = tauri::async_runtime::block_on(get_feedback(
            "https://script.google.com/macros/echo?x=1",
        ))
        .unwrap_err();
        assert!(err.contains("not allowed"), "{err}");
    }

    #[test]
    fn redirect_response_stays_a_post() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let body =
            r#"{"version":"0.3.3","os":"macos aarch64","client":"verge","report":"keep-me"}"#;
        let handle = std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            sock.set_read_timeout(Some(Duration::from_secs(2))).ok();
            let mut buf = Vec::new();
            let mut tmp = [0u8; 2048];
            loop {
                match sock.read(&mut tmp) {
                    Ok(0) => break,
                    Ok(n) => {
                        buf.extend_from_slice(&tmp[..n]);
                        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                            let head = String::from_utf8_lossy(&buf[..pos]).to_lowercase();
                            let len = head
                                .lines()
                                .find_map(|l| l.strip_prefix("content-length:"))
                                .and_then(|v| v.trim().parse::<usize>().ok())
                                .unwrap_or(0);
                            while buf.len() < pos + 4 + len {
                                match sock.read(&mut tmp) {
                                    Ok(0) | Err(_) => break,
                                    Ok(n) => buf.extend_from_slice(&tmp[..n]),
                                }
                            }
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            let req = String::from_utf8_lossy(&buf).into_owned();
            let payload = b"should-not-follow";
            let resp = format!(
                "HTTP/1.1 302 Found\r\nLocation: https://script.googleusercontent.com/macros/echo?x=1\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                payload.len()
            );
            let _ = sock.write_all(resp.as_bytes());
            let _ = sock.write_all(payload);
            let _ = sock.flush();
            req
        });
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .timeout(Duration::from_secs(2))
            .build()
            .unwrap();
        let got = tauri::async_runtime::block_on(exchange_once(
            &client,
            &format!("http://127.0.0.1:{port}/exec"),
            body,
        ))
        .unwrap();
        let req = handle.join().unwrap();
        assert!(req.starts_with("POST /exec"), "{req}");
        assert!(!req.starts_with("GET "), "{req}");
        assert!(req.contains(body), "{req}");
        assert_eq!(got.status, 302);
        assert_eq!(
            got.location.as_deref(),
            Some("https://script.googleusercontent.com/macros/echo?x=1")
        );
        assert_eq!(got.body, "should-not-follow");
    }

    #[test]
    fn echo_result_is_a_get_without_the_json_body() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            sock.set_read_timeout(Some(Duration::from_secs(2))).ok();
            let mut buf = Vec::new();
            let mut tmp = [0u8; 2048];
            loop {
                match sock.read(&mut tmp) {
                    Ok(0) => break,
                    Ok(n) => {
                        buf.extend_from_slice(&tmp[..n]);
                        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            let req = String::from_utf8_lossy(&buf).into_owned();
            let payload = br#"{"ok":true}"#;
            let resp = if req.starts_with("POST ") {
                "HTTP/1.1 405 Method Not Allowed\r\nAllow: HEAD, GET\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    .to_string()
            } else {
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    payload.len()
                )
            };
            let _ = sock.write_all(resp.as_bytes());
            if !req.starts_with("POST ") {
                let _ = sock.write_all(payload);
            }
            let _ = sock.flush();
            req
        });
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .timeout(Duration::from_secs(2))
            .build()
            .unwrap();
        let got = tauri::async_runtime::block_on(exchange_get(
            &client,
            &format!("http://127.0.0.1:{port}/macros/echo?user_content_key=abc&lib=xyz"),
        ))
        .unwrap();
        let req = handle.join().unwrap();
        assert!(
            req.starts_with("GET /macros/echo?user_content_key=abc&lib=xyz "),
            "{req}"
        );
        assert!(!req.contains("keep-me"), "{req}");
        assert!(
            !req.to_lowercase()
                .contains("content-type: application/json"),
            "{req}"
        );
        assert_eq!(got.status, 200);
        assert_eq!(got.body, r#"{"ok":true}"#);
    }
}
