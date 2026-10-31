//! 规则 4 + secret 闸门 + 连接后绑定的端到端测试（Windows CI，进程内 TCP 服务端）。

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use super::*;
use crate::mihomo::trust::{FactErr, Ident, ListenerRow, Who};

fn test_policy() -> TrustPolicy {
    let exe = std::env::current_exe().unwrap();
    let mut p = TrustPolicy::production();
    p.images = vec![exe.file_name().unwrap().to_string_lossy().into_owned()];
    p
}

fn ok_fallback() -> TcpFallback {
    TcpFallback::Allowed { denied_seen: false }
}

/// `None`：从未有连接；`Some(bytes)`：接受了连接并收到这些字节。
type Srv = tauri::async_runtime::JoinHandle<Option<Vec<u8>>>;

fn server() -> (SocketAddr, Srv) {
    let l = tauri::async_runtime::block_on(TcpListener::bind("127.0.0.1:0")).unwrap();
    let addr = l.local_addr().unwrap();
    let h = tauri::async_runtime::spawn(async move {
        let (mut s, _) = match tokio::time::timeout(Duration::from_millis(1500), l.accept()).await {
            Ok(Ok(x)) => x,
            _ => return None,
        };
        let mut got = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            match tokio::time::timeout(Duration::from_secs(2), s.read(&mut buf)).await {
                Ok(Ok(n)) if n > 0 => got.extend_from_slice(&buf[..n]),
                _ => break,
            }
            if got.windows(4).any(|w| w == b"\r\n\r\n") {
                let _ = s
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                    )
                    .await;
                let _ = s.shutdown().await;
                break;
            }
        }
        Some(got)
    });
    (addr, h)
}

fn call(
    policy: &TrustPolicy,
    gate_open: bool,
    fallback: TcpFallback,
    addr: SocketAddr,
    secret: Option<&str>,
) -> Result<(UnixHttpResult, &'static str), String> {
    tauri::async_runtime::block_on(http_via_tcp_with(
        policy, gate_open, fallback, addr, "GET", "/version", None, secret, 3000,
    ))
}

fn finish(h: Srv) -> Option<Vec<u8>> {
    tauri::async_runtime::block_on(h).unwrap()
}

fn assert_never_connected(h: Srv) {
    assert!(finish(h).is_none(), "拒绝时不得建立 TCP 连接");
}

#[test]
fn secret_gate_is_closed_in_m1() {
    // Ellie 时序条件：V5 复跑（S-F2 / S-D′）结论写入 PR 之前不得打开。
    const { assert!(!WINDOWS_TCP_SECRET_GATE_OPEN) };
}

#[test]
fn verified_listener_without_secret_sends_no_authorization() {
    let (addr, h) = server();
    let (res, label) = call(&test_policy(), false, ok_fallback(), addr, None).expect("verified");
    assert_eq!(
        (res.status, res.body.as_str(), label),
        (200, "ok", "verge-tcp(listener-verified)")
    );
    let req = String::from_utf8(finish(h).unwrap())
        .unwrap()
        .to_ascii_lowercase();
    assert!(req.starts_with("get /version http/1.1\r\n"), "{req}");
    assert!(!req.contains("authorization"), "{req}");
}

#[test]
fn verified_listener_with_secret_and_closed_gate_never_connects() {
    let (addr, h) = server();
    let e = call(
        &test_policy(),
        WINDOWS_TCP_SECRET_GATE_OPEN,
        ok_fallback(),
        addr,
        Some("s3cret"),
    )
    .unwrap_err();
    assert!(e.contains("暂未开放"), "{e}");
    assert_never_connected(h);
}

#[test]
fn verified_listener_with_open_gate_sends_bearer() {
    let (addr, h) = server();
    call(&test_policy(), true, ok_fallback(), addr, Some("s3cret")).expect("verified + gate");
    let req = String::from_utf8(finish(h).unwrap()).unwrap();
    assert!(
        req.contains("\r\nAuthorization: Bearer s3cret\r\n"),
        "{req}"
    );
}

#[test]
fn unlisted_image_is_rejected_without_connecting() {
    let (addr, h) = server();
    let e = call(&TrustPolicy::production(), true, ok_fallback(), addr, None).unwrap_err();
    assert!(e.contains("image-mismatch"), "{e}");
    assert_never_connected(h);
}

#[test]
fn system_token_is_unverified_and_refused() {
    let (addr, h) = server();
    let mut p = test_policy();
    p.overrides.l3 = Some(Ok(Ident::new(Who::System, "S-1-5-18")));
    let e = call(&p, true, ok_fallback(), addr, None).unwrap_err();
    assert!(
        e.contains("listener-unverified") && e.contains("服务模式"),
        "{e}"
    );
    assert_never_connected(h);
}

#[test]
fn foreign_token_and_unreadable_token() {
    let (addr, h) = server();
    let mut p = test_policy();
    p.overrides.l3 = Some(Ok(Ident::new(Who::Other, "S-1-5-21-9-9-9-1002")));
    assert!(call(&p, true, ok_fallback(), addr, None)
        .unwrap_err()
        .contains("token-mismatch"));
    assert_never_connected(h);

    let (addr, h) = server();
    let mut p = test_policy();
    p.overrides.l3 = Some(Err(FactErr::new("OpenProcessToken", 5)));
    assert!(call(&p, true, ok_fallback(), addr, None)
        .unwrap_err()
        .contains("listener-unverified"));
    assert_never_connected(h);
}

#[test]
fn second_listener_process_is_a_conflict() {
    let (addr, h) = server();
    let mut p = test_policy();
    p.overrides.extra_listeners_v4 = vec![ListenerRow {
        ip: "0.0.0.0".parse().unwrap(),
        pid: 4242,
    }];
    assert!(call(&p, true, ok_fallback(), addr, None)
        .unwrap_err()
        .contains("listener-conflict"));
    assert_never_connected(h);
}

#[test]
fn m3_blocked_fallback_refuses_tcp() {
    let (addr, h) = server();
    let e = call(
        &test_policy(),
        true,
        TcpFallback::Blocked("pipe untrusted".into()),
        addr,
        None,
    )
    .unwrap_err();
    assert!(e.contains("pipe untrusted"), "{e}");
    assert_never_connected(h);
}

#[test]
fn post_connect_pid_mismatch_writes_zero_bytes() {
    let (addr, h) = server();
    let mut p = test_policy();
    p.overrides.conn_pid = Some(Ok(Some(4242)));
    let e = call(&p, true, ok_fallback(), addr, None).unwrap_err();
    assert!(e.contains("4242"), "{e}");
    assert_eq!(
        finish(h).expect("已连接").len(),
        0,
        "连接后校验失败必须零字节"
    );
}

#[test]
fn real_post_connect_lookup_finds_our_listener() {
    // 不注入 conn_pid：真实连接表必须能找到本进程。
    let (addr, h) = server();
    call(&test_policy(), false, ok_fallback(), addr, None).expect("真实连接表绑定");
    assert!(finish(h).is_some());
}

#[test]
fn loopback_only_hosts() {
    assert_eq!(
        loopback_addr("localhost", 9097).unwrap(),
        "127.0.0.1:9097".parse().unwrap()
    );
    assert_eq!(
        loopback_addr("[::1]", 9097).unwrap(),
        "[::1]:9097".parse().unwrap()
    );
    assert!(loopback_addr("127.0.0.2", 1).is_ok());
    for bad in ["192.168.1.2", "0.0.0.0", "example.com", "", "::"] {
        assert!(loopback_addr(bad, 1).is_err(), "{bad}");
    }
}

/// Ellie PR50 Q3：Verified 但没能持有监听进程句柄（这里 L2 ① / L3 都被注入，真实句柄从未打开）
/// ⇒ 拒绝且不建连。正常路径（上面的 verified 测试）证明真实句柄会被持有到请求结束。
#[test]
fn verified_without_held_process_handle_is_refused_without_connecting() {
    let (addr, h) = server();
    let mut p = test_policy();
    let exe = std::env::current_exe().unwrap();
    p.overrides.l2_1 = Some(Ok(exe.to_string_lossy().into_owned()));
    let me = identity::known().unwrap().me_str.clone();
    p.overrides.l3 = Some(Ok(Ident::new(Who::Me, me)));
    let e = call(&p, true, ok_fallback(), addr, None).unwrap_err();
    assert!(e.contains("句柄"), "{e}");
    assert_never_connected(h);
}

/// Ellie PR50 M-a：单次管道请求不能放宽缓存；完整探测可以。
#[test]
fn pipe_outcome_cache_single_requests_only_tighten() {
    let t0 = Instant::now();
    let allowed = || TcpFallback::Allowed { denied_seen: false };
    let blocked = || TcpFallback::Blocked("untrusted".into());
    let mut c = PipeOutcomeCache::new();

    // 空缓存：单次成功 / NotFound 不写（下一次 TCP 请求仍会做完整探测）
    c.tighten(allowed(), t0);
    assert_eq!(c.get(t0), None);

    // 探测得出 Blocked 后，单次成功 / NotFound / VerifiedOnly 都不能放宽，也不延长 TTL
    c.record_probe(blocked(), t0);
    let t1 = t0 + Duration::from_secs(10);
    c.tighten(allowed(), t1);
    c.tighten(TcpFallback::VerifiedOnly("v".into()), t1);
    assert_eq!(c.get(t1), Some(blocked()));
    assert_eq!(c.get(t0 + PIPE_OUTCOME_TTL), None, "TTL 仍从探测时刻起算");

    // Allowed 之后单次 Untrusted 收紧为 Blocked（并刷新时间戳）
    c.record_probe(allowed(), t0);
    c.tighten(blocked(), t1);
    assert_eq!(c.get(t1 + Duration::from_secs(25)), Some(blocked()));

    // 只有完整探测能放宽
    c.record_probe(allowed(), t1);
    assert_eq!(c.get(t1), Some(allowed()));
}
