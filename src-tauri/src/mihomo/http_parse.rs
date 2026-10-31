//! Raw HTTP response parsing for the Unix-socket transport.
//!
//! The Unix path (see `super::transport`) reads bytes straight off the socket with no
//! auto-decode, so this module owns the byte-level HTTP parsing that `reqwest` does for
//! TCP: header/body split, status line, chunked transfer decoding (by **byte** length,
//! so multi-byte UTF-8 node names never corrupt), and gzip rejection. Kept dependency-
//! free (only `super::types`) so it can be unit-tested with fixtures alone.

use super::types::{UnixHttpResult, MAX_BODY_BYTES};

/// Split HTTP response on bytes (headers/body), decode chunked by byte length, UTF-8 body.
pub(crate) fn parse_http_response_bytes(raw: &[u8]) -> Result<UnixHttpResult, String> {
    let (header_bytes, body_bytes) = split_headers_body(raw)
        .ok_or_else(|| "unix response missing header/body separator".to_string())?;

    let header = String::from_utf8_lossy(header_bytes);
    let status = header
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(0);

    let header_lower = header.to_ascii_lowercase();
    if header_lower
        .lines()
        .any(|l| l.trim().starts_with("content-encoding:") && l.contains("gzip"))
    {
        return Err(format!(
            "unix response Content-Encoding: gzip (raw socket cannot decode); \
             request used Accept-Encoding: identity — body {} bytes",
            body_bytes.len()
        ));
    }

    let decoded = if header_lower
        .lines()
        .any(|l| l.trim().starts_with("transfer-encoding:") && l.contains("chunked"))
    {
        decode_chunked_bytes(body_bytes)?
    } else {
        body_bytes.to_vec()
    };

    if decoded.len() > MAX_BODY_BYTES {
        return Err(format!(
            "unix decoded body too large ({} bytes > {} max)",
            decoded.len(),
            MAX_BODY_BYTES
        ));
    }

    let body_out = String::from_utf8(decoded).map_err(|e| {
        format!(
            "unix response body is not valid UTF-8 after decode: {e} (raw {} bytes)",
            body_bytes.len()
        )
    })?;

    Ok(UnixHttpResult {
        status,
        body: body_out,
    })
}

fn split_headers_body(raw: &[u8]) -> Option<(&[u8], &[u8])> {
    if let Some(pos) = find_bytes(raw, b"\r\n\r\n") {
        return Some((&raw[..pos], &raw[pos + 4..]));
    }
    if let Some(pos) = find_bytes(raw, b"\n\n") {
        return Some((&raw[..pos], &raw[pos + 2..]));
    }
    None
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Decode HTTP/1.1 chunked transfer encoding using **byte** lengths (not chars).
/// Multi-byte UTF-8 (e.g. Chinese node names) must not be counted as one unit per char.
pub(crate) fn decode_chunked_bytes(input: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut rest = input;
    while !rest.is_empty() {
        let Some(nl) = rest.iter().position(|&b| b == b'\n') else {
            out.extend_from_slice(rest);
            break;
        };
        let size_line = &rest[..nl];
        let size_hex = trim_ascii_ws_and_cr(size_line);
        // Ignore chunk extensions after `;`
        let size_hex = size_hex.split(|&b| b == b';').next().unwrap_or(size_hex);
        let size_str = std::str::from_utf8(size_hex)
            .map_err(|_| format!("chunk size line is not ASCII hex: {size_hex:?}"))?;
        let size = usize::from_str_radix(size_str.trim(), 16)
            .map_err(|e| format!("invalid chunk size '{size_str}': {e}"))?;
        rest = &rest[nl + 1..];
        if size == 0 {
            break;
        }
        if rest.len() < size {
            return Err(format!(
                "chunked body truncated: need {size} bytes, have {}",
                rest.len()
            ));
        }
        out.extend_from_slice(&rest[..size]);
        rest = &rest[size..];
        // Trailing CRLF after chunk data
        if rest.starts_with(b"\r\n") {
            rest = &rest[2..];
        } else if rest.starts_with(b"\n") {
            rest = &rest[1..];
        } else if !rest.is_empty() {
            return Err("chunk missing CRLF trailer".into());
        }
    }
    Ok(out)
}

fn trim_ascii_ws_and_cr(s: &[u8]) -> &[u8] {
    let mut start = 0;
    let mut end = s.len();
    while start < end && (s[start] == b' ' || s[start] == b'\t' || s[start] == b'\r') {
        start += 1;
    }
    while end > start && (s[end - 1] == b' ' || s[end - 1] == b'\t' || s[end - 1] == b'\r') {
        end -= 1;
    }
    &s[start..end]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mihomo::nodes::slim_nodes_from_proxies_json;

    // ---- moved from the old monolith: parser end-to-end with Chinese UTF-8 ----

    #[test]
    fn decode_chunked_bytes_roundtrips_multibyte_utf8_chinese_names() {
        // Chunk sizes are in bytes. Chinese chars are 3 bytes each in UTF-8.
        // Old char-based decoder would slice mid-codepoint / corrupt JSON.
        let json =
            r#"{"proxies":{"香港 HK-2-AT":{"type":"Hysteria2"},"日本 TY-4-HY2":{"type":"Vmess"}}}"#;
        let json_bytes = json.as_bytes();
        assert!(json_bytes.len() > 40, "fixture should be multi-chunk sized");

        // Force a split that is NOT on a char boundary to prove byte lengths matter.
        let mid_bad = {
            let mut i = 0;
            while i < json_bytes.len() {
                let c = json[i..].chars().next().unwrap();
                let len = c.len_utf8();
                if len > 1 {
                    break;
                }
                i += len;
            }
            let c = json[i..].chars().next().unwrap();
            assert!(c.len_utf8() > 1);
            i + 1 // inside the multi-byte char
        };
        let (a, b) = json_bytes.split_at(mid_bad);
        assert!(!json.is_char_boundary(mid_bad));

        let mut chunked = Vec::new();
        chunked.extend_from_slice(format!("{:x}\r\n", a.len()).as_bytes());
        chunked.extend_from_slice(a);
        chunked.extend_from_slice(b"\r\n");
        chunked.extend_from_slice(format!("{:x}\r\n", b.len()).as_bytes());
        chunked.extend_from_slice(b);
        chunked.extend_from_slice(b"\r\n");
        chunked.extend_from_slice(b"0\r\n\r\n");

        let decoded = decode_chunked_bytes(&chunked).expect("decode_chunked_bytes");
        let decoded_str = String::from_utf8(decoded).expect("utf8");
        assert_eq!(decoded_str, json);

        let slim = slim_nodes_from_proxies_json(&decoded_str).unwrap();
        let names: Vec<_> = slim.nodes.iter().map(|n| n.name.as_str()).collect();
        assert!(names.contains(&"香港 HK-2-AT"));
        assert!(names.contains(&"日本 TY-4-HY2"));
    }

    #[test]
    fn parse_http_response_chunked_preserves_chinese_json() {
        let json = r#"{"proxies":{"香港节点":{"type":"Shadowsocks"},"DIRECT":{"type":"Direct"}}}"#;
        let json_b = json.as_bytes();
        let mid = json_b.len() / 2;
        // Prefer split inside a multi-byte char if possible
        let split_at = (0..json_b.len())
            .find(|&i| !json.is_char_boundary(i) && i > 10)
            .unwrap_or(mid);
        let (a, b) = json_b.split_at(split_at);

        let mut body = Vec::new();
        body.extend_from_slice(format!("{:X}\r\n", a.len()).as_bytes());
        body.extend_from_slice(a);
        body.extend_from_slice(b"\r\n");
        body.extend_from_slice(format!("{:X}\r\n", b.len()).as_bytes());
        body.extend_from_slice(b);
        body.extend_from_slice(b"\r\n0\r\n\r\n");

        let mut raw = Vec::new();
        raw.extend_from_slice(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n",
        );
        raw.extend_from_slice(&body);

        let res = parse_http_response_bytes(&raw).expect("parse");
        assert_eq!(res.status, 200);
        assert_eq!(res.body, json);
        let slim = slim_nodes_from_proxies_json(&res.body).unwrap();
        assert!(slim.nodes.iter().any(|n| n.name == "香港节点"));
    }

    // ---- new pure-parser fixtures: branches not covered by the monolith tests ----

    #[test]
    fn split_on_lf_lf_when_no_crlf() {
        // LF-only separator exercises the split_headers_body `\n\n` fallback branch.
        let raw = b"HTTP/1.1 204 No Content\n\n";
        let res = parse_http_response_bytes(raw).expect("parse lf-only");
        assert_eq!(res.status, 204);
        assert_eq!(res.body, "");
    }

    #[test]
    fn missing_header_separator_is_an_error() {
        let raw = b"HTTP/1.1 200 OK no blank line here";
        let err = parse_http_response_bytes(raw).expect_err("should fail");
        assert!(err.contains("missing header/body separator"), "{err}");
    }

    #[test]
    fn rejects_gzip_content_encoding() {
        // Raw socket cannot decode gzip; parser must surface a clear error, not garble bytes.
        let raw = b"HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\n\r\nsomebinarybytes";
        let err = parse_http_response_bytes(raw).expect_err("gzip should be rejected");
        assert!(err.contains("gzip"), "{err}");
    }

    #[test]
    fn non_chunked_body_passes_through_verbatim() {
        // No transfer-encoding: chunked → body returned as-is (the `else { body_bytes.to_vec() }`).
        let raw = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n{\"proxies\":{}}";
        let res = parse_http_response_bytes(raw).expect("parse");
        assert_eq!(res.body, "{\"proxies\":{}}");
    }

    #[test]
    fn decodes_chunk_extensions_after_semicolon() {
        // Chunk-size line "6;ext=foo" — the `;` extension must be ignored, size is 6.
        let input = b"6;ext=foo\r\nabcdef\r\n0\r\n\r\n";
        let decoded = decode_chunked_bytes(input).expect("decode with chunk extension");
        assert_eq!(&decoded, b"abcdef");
    }

    #[test]
    fn errors_on_truncated_chunk() {
        // Declares 16 bytes but supplies fewer → the "chunked body truncated" branch.
        let input = b"10\r\nshort\r\n";
        let err = decode_chunked_bytes(input).expect_err("truncated should fail");
        assert!(err.contains("truncated"), "{err}");
    }

    #[test]
    fn errors_on_missing_crlf_trailer() {
        // Chunk data not followed by CRLF/LF, with trailing bytes → "chunk missing CRLF trailer".
        let input = b"3\r\nabcdef";
        let err = decode_chunked_bytes(input).expect_err("missing trailer should fail");
        assert!(err.contains("missing CRLF trailer"), "{err}");
    }

    #[test]
    fn appends_trailing_bytes_without_newline() {
        // A size line with no terminating newline hits the `else { out.extend(rest); break }` guard.
        let decoded = decode_chunked_bytes(b"no-newline-tail").expect("decode");
        assert_eq!(&decoded, b"no-newline-tail");
    }

    #[test]
    fn decodes_multiple_chunks_with_bare_lf_trailers() {
        // Two chunks separated by bare `\n` trailers (not CRLF) must concatenate correctly.
        let input = b"3\nabc\n3\ndef\n0\n\n";
        let decoded = decode_chunked_bytes(input).expect("decode lf trailers");
        assert_eq!(&decoded, b"abcdef");
    }
}
