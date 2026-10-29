//! Bandwidth probe: a timed GET/POST through the optional mixed-port proxy.
//! Streams and counts bytes without buffering the whole body, tolerating a
//! partial read when a sample is cut short by timeout.

use std::time::Duration;

use super::types::{TimedTransferResult, MAX_TIMED_BODY_BYTES};

/// Timed GET/POST via optional mixed-port; counts bytes and drops the body (for bandwidth samples).
pub async fn proxy_timed_transfer_async(
    url: &str,
    mixed_port: Option<u16>,
    method: &str,
    upload_bytes: Option<u64>,
    timeout_ms: u64,
    cancel: Option<super::cancel::CancelWait>,
) -> Result<TimedTransferResult, String> {
    let work = proxy_timed_transfer_inner(url, mixed_port, method, upload_bytes, timeout_ms);
    if let Some(cancel) = cancel {
        return match futures_util::future::select(std::pin::pin!(work), std::pin::pin!(cancel)).await {
            futures_util::future::Either::Left((result, _)) => result,
            futures_util::future::Either::Right((_, _)) => Ok(TimedTransferResult {
                ok: false,
                status: 0,
                bytes: 0,
                elapsed_ms: 0,
                error: Some("已取消".into()),
            }),
        };
    }
    work.await
}

async fn proxy_timed_transfer_inner(
    url: &str,
    mixed_port: Option<u16>,
    method: &str,
    upload_bytes: Option<u64>,
    timeout_ms: u64,
) -> Result<TimedTransferResult, String> {
    use futures_util::StreamExt;
    use std::time::Instant;

    // Bandwidth samples must not auto-decompress: gzip/br decode failures through
    // mixed-port often surface as "error decoding response body" with 0 bytes.
    let mut builder = reqwest::Client::builder()
        .timeout(Duration::from_millis(timeout_ms))
        .redirect(super::guard::probe_redirect_policy(5))
        .danger_accept_invalid_certs(false)
        .gzip(false);

    if let Some(port) = mixed_port {
        let proxy_url = format!("http://127.0.0.1:{port}");
        let proxy = reqwest::Proxy::all(&proxy_url).map_err(|e| format!("proxy: {e}"))?;
        builder = builder.proxy(proxy);
    } else {
        builder = builder.no_proxy();
    }

    let client = builder.build().map_err(|e| format!("client: {e}"))?;
    let method_u = method.to_uppercase();
    let upload_len = upload_bytes.unwrap_or(0).min(MAX_TIMED_BODY_BYTES as u64) as usize;

    let t0 = Instant::now();
    let send_result = match method_u.as_str() {
        "POST" => {
            let body = vec![0u8; upload_len];
            client
                .post(url)
                .header("Content-Type", "application/octet-stream")
                .header("Accept-Encoding", "identity")
                .body(body)
                .send()
                .await
        }
        "GET" | "" => {
            client
                .get(url)
                .header("Accept-Encoding", "identity")
                .header("Cache-Control", "no-cache")
                .send()
                .await
        }
        other => {
            return Ok(TimedTransferResult {
                ok: false,
                status: 0,
                bytes: 0,
                elapsed_ms: 0,
                error: Some(format!("unsupported method: {other}")),
            });
        }
    };

    let res = match send_result {
        Ok(r) => r,
        Err(e) => {
            let elapsed_ms = t0.elapsed().as_millis() as u64;
            return Ok(TimedTransferResult {
                ok: false,
                status: 0,
                bytes: 0,
                elapsed_ms,
                error: Some(format!("request: {e}")),
            });
        }
    };

    let status = res.status().as_u16();
    // Stream + count: discard payload; partial bytes still usable if timeout mid-read.
    let mut stream = res.bytes_stream();
    let mut n: u64 = 0;
    let mut read_err: Option<String> = None;
    while let Some(chunk) = stream.next().await {
        match chunk {
            Ok(bytes) => {
                n = n.saturating_add(bytes.len() as u64);
                if n > MAX_TIMED_BODY_BYTES as u64 {
                    read_err = Some(format!(
                        "response too large ({} > {} max)",
                        n, MAX_TIMED_BODY_BYTES
                    ));
                    break;
                }
            }
            Err(e) => {
                read_err = Some(format!("read body: {e}"));
                break;
            }
        }
    }
    let elapsed_ms = t0.elapsed().as_millis() as u64;

    if method_u == "POST" {
        // Upload sample: count what we sent (response may be tiny / empty).
        let ok = (200..400).contains(&status) && read_err.is_none();
        return Ok(TimedTransferResult {
            ok,
            status,
            bytes: upload_len as u64,
            elapsed_ms,
            error: if ok {
                None
            } else {
                Some(read_err.unwrap_or_else(|| format!("HTTP {status}")))
            },
        });
    }

    // GET download: prefer full body; accept partial if we got meaningful bytes before timeout.
    let ok_full = (200..400).contains(&status) && read_err.is_none() && n > 0;
    let ok_partial = (200..400).contains(&status) && n >= 64 * 1024;
    let ok = ok_full || ok_partial;
    let error = if ok_full {
        None
    } else if ok_partial {
        Some(format!(
            "部分下载 {} B（{}）仍按已收字节估算",
            n,
            read_err.unwrap_or_else(|| "未完整读完".into())
        ))
    } else {
        Some(read_err.unwrap_or_else(|| {
            if n == 0 {
                format!("HTTP {status} · 0 B")
            } else {
                format!("HTTP {status}")
            }
        }))
    };

    Ok(TimedTransferResult {
        ok,
        status,
        bytes: n,
        elapsed_ms,
        error,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smoke_proxy_timed_transfer_dead_proxy_no_panic() {
        let result = std::panic::catch_unwind(|| {
            tauri::async_runtime::block_on(proxy_timed_transfer_async(
                "https://speed.cloudflare.com/__down?bytes=1024",
                Some(1),
                "GET",
                None,
                800,
                None,
            ))
        });
        assert!(result.is_ok(), "proxy_timed_transfer_async panicked");
        let inner = result.unwrap().expect("Result::Ok");
        assert!(!inner.ok, "dead proxy should not report ok: {inner:?}");
    }
}
