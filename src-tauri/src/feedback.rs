//! 诊断报告的保存对话框，以及发往反馈接口的单次 POST。
//!
//! Apps Script 的 exec URL 会 302 到 `script.googleusercontent.com`。reqwest 的默认
//! 重定向会把 301/302/303 的 POST 改成 GET 并丢掉正文，发送就会失败。这里用
//! `redirect::Policy::none()`，把状态码和 Location 交回前端；前端用同一份 JSON 再 POST。

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;

const MAX_TEXT: usize = 256 * 1024;
const MAX_PNG: usize = 4 * 1024 * 1024;
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
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(20))
        .user_agent(format!("clash-assist/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| format!("http client: {e}"))
}

/// 发一次 POST，不跟随重定向。调用方看到 302 时必须自己用同一份 body 再 POST。
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

pub async fn post_feedback(url: &str, body: &str) -> Result<FeedbackExchange, String> {
    allow_feedback_url(url)?;
    let client = feedback_client()?;
    exchange_once(&client, url, body).await
}

pub fn safe_file_name(name: &str, ext: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or("").trim();
    let base = base.trim_matches('.').trim();
    let mut cleaned: String = base
        .chars()
        .filter(|c| !c.is_control() && *c != '/' && *c != '\\')
        .take(80)
        .collect();
    if cleaned.is_empty() {
        cleaned = "clash-assist".into();
    }
    let suffix = format!(".{ext}");
    if cleaned.to_ascii_lowercase().ends_with(&suffix) {
        cleaned
    } else {
        format!("{cleaned}{suffix}")
    }
}

#[cfg(any(test, target_os = "macos", target_os = "windows"))]
pub fn ensure_ext(path: PathBuf, ext: &str) -> PathBuf {
    match path.extension().and_then(|e| e.to_str()) {
        Some(got) if got.eq_ignore_ascii_case(ext) => path,
        _ => path.with_extension(ext),
    }
}

pub fn text_sidecar(png: &Path) -> PathBuf {
    png.with_extension("txt")
}

fn normalize_text_ext(ext: &str) -> Result<&'static str, String> {
    match ext {
        "md" | "markdown" => Ok("md"),
        "txt" => Ok("txt"),
        _ => Err("unsupported file type".into()),
    }
}

pub fn write_bytes(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = crate::safe_fs::create_truncate(path).map_err(|e| format!("save: {e}"))?;
    file.write_all(bytes).map_err(|e| format!("save: {e}"))?;
    file.flush().map_err(|e| format!("save: {e}"))?;
    Ok(())
}

fn decode_b64_char(c: u8) -> Result<u8, String> {
    match c {
        b'A'..=b'Z' => Ok(c - b'A'),
        b'a'..=b'z' => Ok(c - b'a' + 26),
        b'0'..=b'9' => Ok(c - b'0' + 52),
        b'+' => Ok(62),
        b'/' => Ok(63),
        _ => Err("bad base64".into()),
    }
}

pub fn decode_base64(input: &str) -> Result<Vec<u8>, String> {
    let bytes: Vec<u8> = input.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    if bytes.is_empty() || !bytes.len().is_multiple_of(4) {
        return Err("bad base64".into());
    }
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    for chunk in bytes.chunks(4) {
        let pad = chunk.iter().rev().take_while(|c| **c == b'=').count();
        if pad > 2 {
            return Err("bad base64".into());
        }
        let mut n = 0u32;
        for (i, c) in chunk.iter().enumerate() {
            if *c == b'=' {
                if i < 4 - pad {
                    return Err("bad base64".into());
                }
                continue;
            }
            if i >= 4 - pad {
                return Err("bad base64".into());
            }
            n = (n << 6) | u32::from(decode_b64_char(*c)?);
        }
        n <<= 6 * pad as u32;
        out.push((n >> 16) as u8);
        if pad < 2 {
            out.push((n >> 8) as u8);
        }
        if pad < 1 {
            out.push(n as u8);
        }
    }
    Ok(out)
}

pub fn decode_png_base64(input: &str) -> Result<Vec<u8>, String> {
    let bytes = decode_base64(input)?;
    if bytes.len() < 8 || &bytes[..8] != b"\x89PNG\r\n\x1a\n" {
        return Err("not a png".into());
    }
    if bytes.len() > MAX_PNG {
        return Err("png too large".into());
    }
    Ok(bytes)
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn pick_save_path(
    app: &tauri::AppHandle,
    file_name: &str,
    ext: &str,
) -> Result<Option<PathBuf>, String> {
    let label = match ext {
        "md" => "Markdown",
        "png" => "PNG",
        _ => "Text",
    };
    let mut dialog = rfd::FileDialog::new()
        .set_title("Clash Assist")
        .set_file_name(file_name)
        .add_filter(label, &[ext]);
    if let Some(win) = app.get_webview_window("main") {
        dialog = dialog.set_parent(&win);
    }
    Ok(dialog.save_file().map(|path| ensure_ext(path, ext)))
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn pick_save_path(
    app: &tauri::AppHandle,
    file_name: &str,
    ext: &str,
) -> Result<Option<PathBuf>, String> {
    let _ = (app, file_name, ext);
    Err("save dialog is unavailable".into())
}

pub fn save_text_document(
    app: &tauri::AppHandle,
    default_name: &str,
    content: &str,
    extension: &str,
) -> Result<Option<String>, String> {
    if content.len() > MAX_TEXT {
        return Err("report too large".into());
    }
    let ext = normalize_text_ext(extension)?;
    let name = safe_file_name(default_name, ext);
    let Some(path) = pick_save_path(app, &name, ext)? else {
        return Ok(None);
    };
    write_bytes(&path, content.as_bytes())?;
    Ok(Some(path.display().to_string()))
}

pub fn save_png_document(
    app: &tauri::AppHandle,
    default_name: &str,
    png_base64: &str,
    sidecar_text: &str,
) -> Result<Option<String>, String> {
    if sidecar_text.len() > MAX_TEXT {
        return Err("summary too large".into());
    }
    let bytes = decode_png_base64(png_base64)?;
    let name = safe_file_name(default_name, "png");
    let Some(path) = pick_save_path(app, &name, "png")? else {
        return Ok(None);
    };
    write_bytes(&path, &bytes)?;
    let side = text_sidecar(&path);
    if let Err(e) = write_bytes(&side, sidecar_text.as_bytes()) {
        return Err(format!("text summary was not saved: {e}"));
    }
    Ok(Some(path.display().to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feedback_url_allowlist() {
        let ok = "https://script.google.com/macros/s/AKfycbypPAXzah4RI1px9Wybr9maQI0pJUqhIYuPFeNkJjmy1waiLS9rOrBudcfpysBAqs10rA/exec";
        assert!(allow_feedback_url(ok).is_ok());
        assert!(allow_feedback_url("https://script.googleusercontent.com/macros/echo?x=1").is_ok());
        for bad in [
            "http://script.google.com/macros/s/abc/exec",
            "https://script.google.com/macros/s/abc/exec?x=1",
            "https://user:pw@script.google.com/macros/s/abc/exec",
            "https://script.google.com:443/macros/s/abc/exec",
            "https://evil.example/macros/s/abc/exec",
            "https://script.google.com/other",
            "https://script.google.com.evil.example/macros/s/abc/exec",
        ] {
            assert!(allow_feedback_url(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn file_name_drops_directories_and_keeps_extension() {
        assert_eq!(safe_file_name("../../etc/passwd", "md"), "passwd.md");
        assert_eq!(
            safe_file_name("Clash-Assist-diagnostic.md", "md"),
            "Clash-Assist-diagnostic.md"
        );
        assert_eq!(safe_file_name("..", "txt"), "clash-assist.txt");
        let png = PathBuf::from("/tmp/result.png");
        assert_eq!(text_sidecar(&png), PathBuf::from("/tmp/result.txt"));
        assert_eq!(
            ensure_ext(PathBuf::from("a"), "png"),
            PathBuf::from("a.png")
        );
    }

    #[test]
    fn base64_and_png_magic() {
        assert_eq!(decode_base64("aGk=").unwrap(), b"hi");
        assert!(decode_png_base64("aGk=").is_err());
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend_from_slice(&[0, 1, 2, 3]);
        let b64 = base64_encode(&png);
        assert_eq!(decode_png_base64(&b64).unwrap(), png);
    }

    fn base64_encode(bytes: &[u8]) -> String {
        const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        let mut i = 0;
        while i + 3 <= bytes.len() {
            let n = (u32::from(bytes[i]) << 16)
                | (u32::from(bytes[i + 1]) << 8)
                | u32::from(bytes[i + 2]);
            out.push(T[((n >> 18) & 63) as usize] as char);
            out.push(T[((n >> 12) & 63) as usize] as char);
            out.push(T[((n >> 6) & 63) as usize] as char);
            out.push(T[(n & 63) as usize] as char);
            i += 3;
        }
        if bytes.len() - i == 1 {
            let n = u32::from(bytes[i]) << 16;
            out.push(T[((n >> 18) & 63) as usize] as char);
            out.push(T[((n >> 12) & 63) as usize] as char);
            out.push('=');
            out.push('=');
        } else if bytes.len() - i == 2 {
            let n = (u32::from(bytes[i]) << 16) | (u32::from(bytes[i + 1]) << 8);
            out.push(T[((n >> 18) & 63) as usize] as char);
            out.push(T[((n >> 12) & 63) as usize] as char);
            out.push(T[((n >> 6) & 63) as usize] as char);
            out.push('=');
        }
        out
    }

    #[test]
    fn write_bytes_roundtrip() {
        let dir = std::env::temp_dir().join(format!("ca-feedback-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("note.md");
        write_bytes(&path, "hello".as_bytes()).unwrap();
        assert_eq!(crate::safe_fs::read_to_string(&path).unwrap(), "hello");
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn redirect_response_stays_a_post() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let body =
            r#"{"version":"0.3.2","os":"macos aarch64","client":"verge","report":"keep-me"}"#;
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
}
