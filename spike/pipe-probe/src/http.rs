//! Minimal HTTP/1.1 request/response handling for `GET /version` over a pipe.
//! Only what the spike needs: status line, headers, Content-Length or chunked body.

use serde_json::{json, Value};

/// Hard cap on bytes read from the server.
pub const MAX_RESPONSE_BYTES: usize = 64 * 1024;

pub fn version_request() -> &'static [u8] {
    b"GET /version HTTP/1.1\r\nHost: localhost\r\nUser-Agent: pipe-probe\r\nAccept: */*\r\nConnection: close\r\n\r\n"
}

#[derive(Debug, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub reason: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    pub fn to_json(&self) -> Value {
        let body = String::from_utf8_lossy(&self.body).into_owned();
        let body_json = serde_json::from_slice::<Value>(&self.body).ok();
        json!({
            "status": self.status,
            "reason": self.reason,
            "headers": self.headers.iter().map(|(k, v)| json!([k, v])).collect::<Vec<_>>(),
            "body": body,
            "body_json": body_json,
        })
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Parse {
    /// Need more bytes (or EOF to finish a close-delimited body).
    Incomplete,
    Complete(Response),
    Invalid(String),
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Parse `buf`. `eof` = the peer closed the stream (close-delimited bodies complete then).
pub fn parse_response(buf: &[u8], eof: bool) -> Parse {
    let Some(head_end) = find(buf, b"\r\n\r\n") else {
        return if eof {
            Parse::Invalid(format!(
                "connection closed after {} bytes without a complete header",
                buf.len()
            ))
        } else {
            Parse::Incomplete
        };
    };
    let head = String::from_utf8_lossy(&buf[..head_end]);
    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap_or_default();
    let mut parts = status_line.splitn(3, ' ');
    let proto = parts.next().unwrap_or_default();
    if !proto.starts_with("HTTP/1.") {
        return Parse::Invalid(format!("bad status line: {status_line:?}"));
    }
    let Some(status) = parts.next().and_then(|s| s.parse::<u16>().ok()) else {
        return Parse::Invalid(format!("bad status code: {status_line:?}"));
    };
    let reason = parts.next().unwrap_or_default().to_string();
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect();
    let header = |name: &str| {
        headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    };
    let rest = &buf[head_end + 4..];
    let mk = |body: Vec<u8>| {
        Parse::Complete(Response {
            status,
            reason: reason.clone(),
            headers: headers.clone(),
            body,
        })
    };

    if header("transfer-encoding").is_some_and(|v| v.to_ascii_lowercase().contains("chunked")) {
        return match decode_chunked(rest) {
            Ok(Some(body)) => mk(body),
            Ok(None) if eof => Parse::Invalid("connection closed inside chunked body".into()),
            Ok(None) => Parse::Incomplete,
            Err(e) => Parse::Invalid(e),
        };
    }
    if let Some(cl) = header("content-length") {
        let Ok(n) = cl.parse::<usize>() else {
            return Parse::Invalid(format!("bad content-length {cl:?}"));
        };
        if rest.len() >= n {
            return mk(rest[..n].to_vec());
        }
        return if eof {
            Parse::Invalid(format!(
                "connection closed after {} of {n} body bytes",
                rest.len()
            ))
        } else {
            Parse::Incomplete
        };
    }
    if status == 204 || status == 304 || (100..200).contains(&status) {
        return mk(Vec::new());
    }
    if eof {
        mk(rest.to_vec())
    } else {
        Parse::Incomplete
    }
}

/// `Ok(None)` = need more data.
fn decode_chunked(mut b: &[u8]) -> Result<Option<Vec<u8>>, String> {
    let mut out = Vec::new();
    loop {
        let Some(eol) = find(b, b"\r\n") else {
            return Ok(None);
        };
        let size_str = String::from_utf8_lossy(&b[..eol]);
        let size_str = size_str.split(';').next().unwrap_or_default().trim();
        let size = usize::from_str_radix(size_str, 16)
            .map_err(|_| format!("bad chunk size {size_str:?}"))?;
        b = &b[eol + 2..];
        if size == 0 {
            // Trailers (ignored) end with an empty line.
            return Ok(find(b, b"\r\n").map(|_| out));
        }
        if b.len() < size + 2 {
            return Ok(None);
        }
        out.extend_from_slice(&b[..size]);
        b = &b[size + 2..];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_length() {
        let r = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 16\r\n\r\n{\"version\":\"v1\"}X";
        assert_eq!(parse_response(&r[..30], false), Parse::Incomplete);
        let Parse::Complete(resp) = parse_response(r, false) else {
            panic!()
        };
        assert_eq!(resp.status, 200);
        assert_eq!(resp.body, b"{\"version\":\"v1\"}X"[..16].to_vec());
        assert_eq!(resp.to_json()["body_json"]["version"], json!("v1"));
    }

    #[test]
    fn chunked() {
        let r = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\nabcd\r\n2\r\nef\r\n0\r\n\r\n";
        let Parse::Complete(resp) = parse_response(r, false) else {
            panic!()
        };
        assert_eq!(resp.body, b"abcdef");
        assert_eq!(parse_response(&r[..r.len() - 2], false), Parse::Incomplete);
    }

    #[test]
    fn close_delimited_and_invalid() {
        let r = b"HTTP/1.1 503 Service Unavailable\r\n\r\nsquat";
        assert_eq!(parse_response(r, false), Parse::Incomplete);
        let Parse::Complete(resp) = parse_response(r, true) else {
            panic!()
        };
        assert_eq!((resp.status, resp.body.as_slice()), (503, &b"squat"[..]));
        assert!(matches!(
            parse_response(b"garbage\r\n\r\n", false),
            Parse::Invalid(_)
        ));
        assert!(matches!(
            parse_response(b"HTTP/1.1 200", true),
            Parse::Invalid(_)
        ));
    }
}
