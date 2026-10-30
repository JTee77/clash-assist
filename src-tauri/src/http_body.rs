//! Shared capped body reader for reqwest responses.
//!
//! Used by the Mihomo controller transport and the GitHub update client so a
//! malicious / oversized response cannot be fully buffered before the size
//! check runs.

/// Read a response body while enforcing `max` bytes.
///
/// Declared `Content-Length` above the cap is rejected up front. Otherwise the
/// body is streamed chunk by chunk and aborted as soon as the cumulative size
/// would exceed `max`.
pub async fn read_body_capped(mut res: reqwest::Response, max: usize) -> Result<Vec<u8>, String> {
    let too_large = |n: u64| format!("response body too large ({n} bytes > {max} max)");
    if let Some(len) = res.content_length() {
        if len > max as u64 {
            return Err(too_large(len));
        }
    }
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = res.chunk().await.map_err(|e| format!("read body: {e}"))? {
        if buf.len() + chunk.len() > max {
            return Err(too_large((buf.len() + chunk.len()) as u64));
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn fetch_capped(port: u16, max: usize) -> Result<Vec<u8>, String> {
        tauri::async_runtime::block_on(async move {
            let client = reqwest::Client::builder().no_proxy().build().unwrap();
            let res = client
                .get(format!("http://127.0.0.1:{port}/"))
                .send()
                .await
                .map_err(|e| e.to_string())?;
            read_body_capped(res, max).await
        })
    }

    #[test]
    fn body_cap_enforced_while_streaming_chunked() {
        let mut resp =
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n".to_vec();
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
        let resp =
            b"HTTP/1.1 200 OK\r\nContent-Length: 999999\r\nConnection: close\r\n\r\nabc".to_vec();
        let (port, h) = serve_once(resp);
        let r = fetch_capped(port, 1024);
        let _ = h.join();
        assert!(r
            .expect_err("declared oversize must be rejected")
            .contains("too large"));
    }

    #[test]
    fn body_under_cap_is_returned() {
        let resp =
            b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello".to_vec();
        let (port, h) = serve_once(resp);
        let r = fetch_capped(port, 1024);
        let _ = h.join();
        assert_eq!(r.unwrap(), b"hello");
    }
}
