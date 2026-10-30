//! `pipe <name>`: client-side probe of a Verge-style control pipe (S-A..S-E, P1–P7).
//!
//! Order (plan §4.1 step 4 / §2.2): open with SECURITY_ANONYMOUS → L1 owner → DACL →
//! server PID → L2 ①②③ → L3 → WTS → only then the first byte (`GET /version`).

use std::os::windows::io::AsRawHandle;
use std::time::{Duration, Instant};

use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeClient};
use windows_sys::Win32::Foundation::{ERROR_BROKEN_PIPE, ERROR_PIPE_BUSY};
use windows_sys::Win32::Storage::FileSystem::SECURITY_ANONYMOUS;
use windows_sys::Win32::System::Pipes::GetNamedPipeServerProcessId;

use super::context::Context;
use super::process::{identity_steps, WtsSnapshot};
use super::security;
use super::util::WinErr;
use crate::http::{self, Parse};
use crate::report::{Report, Step};

const BUSY_RETRY: Duration = Duration::from_millis(50);

pub async fn run(name: &str, timeout: Duration, ctx: &Context, report: &mut Report) {
    // --- open --------------------------------------------------------------------------
    let deadline = Instant::now() + timeout;
    let mut attempts = 0u32;
    let client = loop {
        attempts += 1;
        // tokio ORs in SECURITY_SQOS_PRESENT; SECURITY_ANONYMOUS == 0 is the impersonation level.
        match ClientOptions::new()
            .security_qos_flags(SECURITY_ANONYMOUS)
            .open(name)
        {
            Ok(c) => break Ok(c),
            Err(e)
                if e.raw_os_error() == Some(ERROR_PIPE_BUSY as i32)
                    && Instant::now() < deadline =>
            {
                tokio::time::sleep(BUSY_RETRY).await;
            }
            Err(e) => break Err(e),
        }
    };
    let open_info = json!({
        "name": name,
        "attempts": attempts,
        "security_qos": "SECURITY_SQOS_PRESENT|SECURITY_ANONYMOUS",
    });
    let client: NamedPipeClient = match client {
        Ok(c) => {
            report.push(Step::ok("open", open_info));
            c
        }
        Err(e) => {
            let step = match e.raw_os_error() {
                Some(code) => Step::win32("open", "CreateFileW", code as u32),
                None => Step::failed("open", e.to_string()),
            };
            report.push(step.with_value(open_info));
            return;
        }
    };
    let h = client.as_raw_handle();

    // --- L1 owner + DACL (same handle) ----------------------------------------------------
    report.push(match security::owner(h) {
        Ok(sid) => Step::ok("owner", json!({ "owner": ctx.known.describe(&sid) })),
        Err(e) => e.step("owner"),
    });
    report.push(match security::dacl(h, &ctx.known) {
        Ok(v) => Step::ok("dacl", v),
        Err(e) => e.step("dacl"),
    });

    // --- server PID, L2 ①②③, L3, WTS ---------------------------------------------------
    let mut pid = 0u32;
    // SAFETY: valid pipe handle owned by `client`; out-param valid.
    if unsafe { GetNamedPipeServerProcessId(h, &mut pid) } != 0 {
        report.push(Step::ok("server_pid", json!(pid)).with_pid(pid));
        let wts = WtsSnapshot::take();
        for s in identity_steps(pid, &ctx.known, &wts) {
            report.push(s);
        }
    } else {
        report.push(WinErr::last("GetNamedPipeServerProcessId").step("server_pid"));
        for s in ["l2_1", "l2_2", "l2_3", "l3", "wts"] {
            report.push(Step::failed(s, "skipped: no server pid"));
        }
    }

    // --- first byte: GET /version ----------------------------------------------------------
    report.push(http_version(client, timeout).await);
}

async fn http_version(mut client: NamedPipeClient, timeout: Duration) -> Step {
    const STEP: &str = "http_version";
    let started = Instant::now();
    let fut = async {
        if let Err(e) = client.write_all(http::version_request()).await {
            return Err(match e.raw_os_error() {
                Some(c) => Step::win32(STEP, "WriteFile", c as u32),
                None => Step::failed(STEP, format!("write: {e}")),
            }
            .with_value(json!({ "request_sent": false })));
        }
        let mut buf: Vec<u8> = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            let eof = match client.read(&mut chunk).await {
                Ok(0) => true,
                Ok(n) => {
                    buf.extend_from_slice(&chunk[..n]);
                    false
                }
                Err(e) if e.raw_os_error() == Some(ERROR_BROKEN_PIPE as i32) => true,
                Err(e) => {
                    return Err(match e.raw_os_error() {
                        Some(c) => Step::win32(STEP, "ReadFile", c as u32),
                        None => Step::failed(STEP, format!("read: {e}")),
                    }
                    .with_value(json!({ "request_sent": true, "bytes_read": buf.len() })));
                }
            };
            match http::parse_response(&buf, eof) {
                Parse::Complete(r) => return Ok((r, buf.len())),
                Parse::Invalid(msg) => {
                    return Err(Step::failed(STEP, msg).with_value(json!({
                        "request_sent": true,
                        "bytes_read": buf.len(),
                        "raw_prefix": String::from_utf8_lossy(&buf[..buf.len().min(512)]),
                    })))
                }
                // parse_response never reports Incomplete at EOF; guard anyway so we cannot spin.
                Parse::Incomplete if eof => {
                    return Err(Step::failed(
                        STEP,
                        "connection closed before the response completed",
                    )
                    .with_value(json!({ "request_sent": true, "bytes_read": buf.len() })));
                }
                Parse::Incomplete if buf.len() > http::MAX_RESPONSE_BYTES => {
                    return Err(Step::failed(STEP, "response exceeds 64 KiB cap")
                        .with_value(json!({ "request_sent": true, "bytes_read": buf.len() })));
                }
                Parse::Incomplete => {}
            }
        }
    };
    match tokio::time::timeout(timeout, fut).await {
        Ok(Ok((resp, n))) => {
            let mut v = resp.to_json();
            v["bytes_read"] = json!(n);
            v["elapsed_ms"] = json!(started.elapsed().as_millis() as u64);
            Step::ok(STEP, v)
        }
        Ok(Err(step)) => step,
        Err(_) => Step::failed(STEP, format!("timeout after {} ms", timeout.as_millis()))
            .with_value(json!({ "request_sent": true })),
    }
}
