//! `squat <name> <sddl|->`: create a pipe server the way an attacker (`ca_b`) would and record
//! whether the owner can be forged (S-E ⑤⑥, S-R9; expected ERROR_INVALID_OWNER = 1307), and
//! what owner a client would actually see (P7).

use std::os::windows::io::AsRawHandle;
use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
use windows_sys::Win32::System::Pipes::GetNamedPipeClientProcessId;

use super::context::Context;
use super::security::{self, ParsedSd};
use super::sid::OwnedSid;
use super::util::WinErr;
use crate::report::{Report, Step};

/// What the squatter answers to any request, so a `pipe` probe can tell it reached the squatter.
const SQUAT_RESPONSE: &[u8] = b"HTTP/1.1 503 Service Unavailable\r\nContent-Type: application/json\r\nContent-Length: 24\r\nConnection: close\r\n\r\n{\"pipe_probe\":\"squat\"}\r\n";
const CLIENT_IO_TIMEOUT: Duration = Duration::from_secs(3);
const RETRY_PAUSE: Duration = Duration::from_millis(100);

pub struct Args<'a> {
    pub name: &'a str,
    pub sddl: Option<&'a str>,
    pub set_owner: &'a [String],
    pub hold: Duration,
    pub ready_file: Option<&'a Path>,
}

/// `checkpoint` is called whenever the report reaches a state worth persisting
/// (so killing the squatter mid-hold still leaves evidence in `--out`).
pub async fn run(args: Args<'_>, ctx: &Context, report: &mut Report, checkpoint: &dyn Fn(&Report)) {
    // --- security descriptor ---------------------------------------------------------------
    let sd = match args.sddl {
        None => None,
        Some(s) => match ParsedSd::parse(s) {
            Ok(sd) => {
                report.push(Step::ok("sddl_parse", json!({ "sddl": s })));
                Some(sd)
            }
            Err(e) => {
                report.push(e.step("sddl_parse").with_value(json!({ "sddl": s })));
                return;
            }
        },
    };

    // --- create ------------------------------------------------------------------------------
    let mut opts = ServerOptions::new();
    opts.first_pipe_instance(true)
        .reject_remote_clients(true)
        .max_instances(1)
        // WRITE_OWNER on our own handle so SetSecurityInfo(OWNER) fails only for the
        // owner value itself (1307), never for lack of handle access (5).
        .write_owner(true);
    let created = match &sd {
        Some(sd) => {
            let mut sa = SECURITY_ATTRIBUTES {
                nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: sd.as_ptr(),
                bInheritHandle: 0,
            };
            // SAFETY: `sa` is a valid SECURITY_ATTRIBUTES whose descriptor (`sd`) outlives the call.
            unsafe {
                opts.create_with_security_attributes_raw(
                    args.name,
                    (&mut sa as *mut SECURITY_ATTRIBUTES).cast(),
                )
            }
        }
        None => opts.create(args.name),
    };
    let create_info = json!({
        "name": args.name,
        "security": if args.sddl.is_some() { "sddl" } else { "default" },
        "first_pipe_instance": true,
        "write_owner": true,
    });
    let mut server = match created {
        Ok(s) => {
            report.push(Step::ok("create", create_info));
            s
        }
        Err(e) => {
            report.push(
                match e.raw_os_error() {
                    Some(c) => Step::win32("create", "CreateNamedPipeW", c as u32),
                    None => Step::failed("create", e.to_string()),
                }
                .with_value(create_info),
            );
            return;
        }
    };
    let h = server.as_raw_handle();
    report.push(owner_readback(h, ctx, None));

    // --- SetSecurityInfo(OWNER) per target ----------------------------------------------------
    for target in args.set_owner {
        match OwnedSid::parse(target) {
            Err(e) => report.push(e.step("set_owner").with_target(target.clone())),
            Ok(sid) => {
                let desc = ctx.known.describe(&sid);
                report.push(
                    match security::set_owner(h, &sid) {
                        Ok(()) => Step::ok("set_owner", json!({ "requested_owner": desc })),
                        Err(e) => e
                            .step("set_owner")
                            .with_value(json!({ "requested_owner": desc })),
                    }
                    .with_target(target.clone()),
                );
                report.push(owner_readback(h, ctx, Some(target)));
            }
        }
    }

    // --- hold ----------------------------------------------------------------------------------
    checkpoint(report);
    if let Some(p) = args.ready_file {
        let body = json!({ "name": args.name, "pid": std::process::id() }).to_string();
        report.push(match crate::report::write_file_atomic(p, &body) {
            Ok(()) => Step::ok("ready_file", json!({ "path": p.display().to_string() })),
            Err(e) => Step::failed("ready_file", e.to_string()),
        });
    }
    if args.hold.is_zero() {
        return;
    }
    let started = Instant::now();
    let connections = serve(&mut server, args.hold, report, checkpoint).await;
    report.push(Step::ok(
        "hold",
        json!({ "seconds": args.hold.as_secs(), "elapsed_ms": started.elapsed().as_millis() as u64,
                "connections": connections }),
    ));
}

fn owner_readback(
    h: windows_sys::Win32::Foundation::HANDLE,
    ctx: &Context,
    target: Option<&String>,
) -> Step {
    let s = match security::owner(h) {
        Ok(sid) => Step::ok(
            "owner_readback",
            json!({ "owner": ctx.known.describe(&sid) }),
        ),
        Err(e) => e.step("owner_readback"),
    };
    match target {
        Some(t) => s.with_target(t.clone()),
        None => s,
    }
}

/// Accept clients until the hold deadline; answer each with SQUAT_RESPONSE.
async fn serve(
    server: &mut NamedPipeServer,
    hold: Duration,
    report: &mut Report,
    checkpoint: &dyn Fn(&Report),
) -> Vec<Value> {
    let deadline = tokio::time::Instant::now() + hold;
    let mut out = Vec::new();
    loop {
        let connected = match tokio::time::timeout_at(deadline, server.connect()).await {
            Err(_) => break,
            Ok(r) => r,
        };
        if let Err(e) = connected {
            out.push(json!({ "connect_error": io_err_json(&e, "ConnectNamedPipe") }));
            let _ = server.disconnect();
            tokio::time::sleep(RETRY_PAUSE).await;
            continue;
        }
        let mut client_pid = 0u32;
        // SAFETY: valid connected server handle; out-param valid.
        let client_pid =
            if unsafe { GetNamedPipeClientProcessId(server.as_raw_handle(), &mut client_pid) } != 0
            {
                json!(client_pid)
            } else {
                json!({ "error": WinErr::last("GetNamedPipeClientProcessId").to_json() })
            };
        let exchange = exchange(server).await;
        // `exchange` already waited for the client to close, so nothing unread is discarded.
        let disc = server
            .disconnect()
            .err()
            .map(|e| io_err_json(&e, "DisconnectNamedPipe"));
        out.push(
            json!({ "client_pid": client_pid, "exchange": exchange, "disconnect_error": disc }),
        );
        report.push(Step::ok(
            "client_connected",
            out.last().cloned().unwrap_or(Value::Null),
        ));
        checkpoint(report);
    }
    out
}

/// Read the request head (bounded), answer, then wait for the client to close so the
/// response is not discarded by DisconnectNamedPipe.
async fn exchange(server: &mut NamedPipeServer) -> Value {
    let mut req = Vec::new();
    let mut buf = [0u8; 4096];
    let read = tokio::time::timeout(CLIENT_IO_TIMEOUT, async {
        while req.len() < 16 * 1024 && !req.windows(4).any(|w| w == b"\r\n\r\n") {
            match server.read(&mut buf).await? {
                0 => break,
                n => req.extend_from_slice(&buf[..n]),
            }
        }
        Ok::<_, std::io::Error>(())
    })
    .await;
    let first_line: String = String::from_utf8_lossy(&req)
        .lines()
        .next()
        .unwrap_or_default()
        .chars()
        .take(200)
        .collect();
    let read_err = match read {
        Err(_) => Some(json!("timeout")),
        Ok(Err(e)) => Some(io_err_json(&e, "ReadFile")),
        Ok(Ok(())) => None,
    };
    let write = tokio::time::timeout(CLIENT_IO_TIMEOUT, async {
        server.write_all(SQUAT_RESPONSE).await?;
        server.flush().await
    })
    .await;
    let write_err = match write {
        Err(_) => Some(json!("timeout")),
        Ok(Err(e)) => Some(io_err_json(&e, "WriteFile")),
        Ok(Ok(())) => None,
    };
    // Bounded wait for the client to close its end (EOF / broken pipe), discarding extra input.
    let _ = tokio::time::timeout(CLIENT_IO_TIMEOUT, async {
        while let Ok(n) = server.read(&mut buf).await {
            if n == 0 {
                break;
            }
        }
    })
    .await;
    json!({
        "request_bytes": req.len(),
        "request_line": first_line,
        "read_error": read_err,
        "responded": write_err.is_none(),
        "write_error": write_err,
    })
}

fn io_err_json(e: &std::io::Error, api: &'static str) -> Value {
    match e.raw_os_error() {
        Some(c) => WinErr::code(api, c as u32).to_json(),
        None => json!({ "api": api, "error": e.to_string() }),
    }
}
