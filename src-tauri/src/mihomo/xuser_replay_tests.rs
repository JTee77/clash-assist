//! MUST-4（Ellie V5 复跑复核 §2-4、§2-11）跨用户回放的客户端一侧。
//!
//! 这里的测试平时全部忽略，只由 CI 的 `scripts/ci-m1-xuser-replay.ps1` 运行：脚本把本测试
//! 二进制复制给标准用户 ca_a，以 ca_a 身份执行 `--ignored --exact <测试名>`，参数经环境变量
//! 传入。每个结果打印成一行 `CA_XUSER_RESULT <json>` 供脚本解析，测试本身也断言期望结果。
//! 服务端（ca_a / ca_b 的管道实例、ca_b 的 verge-mihomo.exe）都由脚本启动，并在服务端统计
//! 是否接受了连接、收到多少字节。
//!
//! 只调用生产入口（`pipe::http_via_pipe_with`、`win_tcp::check_listener`）和生产
//! `TrustPolicy`，不使用任何注入点。

use std::net::SocketAddr;
use std::time::Duration;

use super::guard::PipeName;
use super::pipe::http_via_pipe_with;
use super::trust::{ListenerCheck, PipeErr, PipeKind, TcpReject, TrustPolicy};
use super::win_tcp::check_listener;

fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("缺少环境变量 {name}"))
}

/// S-D′：ca_a 以 mihomo 默认 SDDL 建管道、ca_b 挂第二实例。连 `CA_XUSER_CONNECTS` 次
///（默认 8），每次都必须是 InsecureDacl / dacl-allows-foreign-instance（不写任何字节）。
#[test]
#[ignore = "仅供跨用户 CI 回放（scripts/ci-m1-xuser-replay.ps1）以标准用户身份运行"]
fn xuser_s_d_prime_client() {
    let name = PipeName::for_test(env("CA_XUSER_PIPE"));
    let connects: usize = std::env::var("CA_XUSER_CONNECTS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(8);
    let policy = TrustPolicy::production();
    let mut rows = Vec::new();
    for i in 0..connects {
        let r = tauri::async_runtime::block_on(http_via_pipe_with(
            &name,
            PipeKind::Sidecar,
            &policy,
            "GET",
            "/version",
            None,
            3000,
        ));
        let row = match r {
            Ok((res, t)) => serde_json::json!({
                "attempt": i, "outcome": "ok", "status": res.status,
                "ownerSid": t.owner.sid, "warnings": t.warnings,
            }),
            Err(e) => {
                let (outcome, u) = match &e {
                    PipeErr::InsecureDacl(u) => ("InsecureDacl", Some(u)),
                    PipeErr::Untrusted(u) => ("Untrusted", Some(u)),
                    PipeErr::NotFound => ("NotFound", None),
                    PipeErr::Busy => ("Busy", None),
                    PipeErr::Denied => ("Denied", None),
                    PipeErr::Io(_) => ("Io", None),
                };
                serde_json::json!({
                    "attempt": i, "outcome": outcome,
                    "reason": u.map(|u| u.reason),
                    "ownerSid": u.and_then(|u| u.owner_sid.clone()),
                    "serverPid": u.and_then(|u| u.server_pid),
                    "message": e.to_string(),
                })
            }
        };
        println!("CA_XUSER_RESULT {row}");
        rows.push(row);
        // 让服务端处理完断开、重新进入等待，再连下一次。
        std::thread::sleep(Duration::from_millis(500));
    }
    for row in &rows {
        assert_eq!(row["outcome"], "InsecureDacl", "{row}");
        assert_eq!(row["reason"], "dacl-allows-foreign-instance", "{row}");
    }
}

/// 规则 4：监听者是 ca_b 的 verge-mihomo.exe（`CA_XUSER_TCP_ADDR`）。结果码必须等于
/// `CA_XUSER_TCP_EXPECT`：Credential↔Credential 为 token-mismatch（共享 logon SID，L3 读得到
/// ca_b 的令牌），ScheduledTask 为 unverified（独立 logon，OpenProcess 返回 5）。只判定，不建连。
#[test]
#[ignore = "仅供跨用户 CI 回放（scripts/ci-m1-xuser-replay.ps1）以标准用户身份运行"]
fn xuser_tcp_listener_check() {
    let addr: SocketAddr = env("CA_XUSER_TCP_ADDR")
        .parse()
        .expect("CA_XUSER_TCP_ADDR 必须是 ip:port");
    let expect = env("CA_XUSER_TCP_EXPECT");
    let row = match check_listener(&TrustPolicy::production(), addr) {
        ListenerCheck::Verified { pid, image } => {
            serde_json::json!({ "code": "verified", "pid": pid, "image": image })
        }
        ListenerCheck::Unverified {
            pid,
            reasons,
            service_mode,
        } => serde_json::json!({
            "code": "unverified", "pid": pid, "reasons": reasons, "serviceMode": service_mode,
        }),
        ListenerCheck::Rejected(r) => {
            let (pid, sid) = match &r {
                TcpReject::TokenMismatch { pid, sid } => (Some(*pid), Some(sid.clone())),
                TcpReject::ImageMismatch { pid, .. } => (Some(*pid), None),
                TcpReject::ListenerConflict(_) => (None, None),
            };
            serde_json::json!({
                "code": r.code(), "pid": pid, "sid": sid, "message": r.to_string(),
            })
        }
        ListenerCheck::NotListening {
            ipv6_dual_stack_hint,
        } => serde_json::json!({
            "code": "not-listening", "ipv6DualStackHint": ipv6_dual_stack_hint,
        }),
    };
    println!("CA_XUSER_RESULT {row}");
    assert_eq!(row["code"], expect.as_str(), "{row}");
}
