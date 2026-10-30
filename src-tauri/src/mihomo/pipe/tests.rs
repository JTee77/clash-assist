//! 真实命名管道上的端到端测试（Windows CI）。服务端由测试进程自己创建：属主 / PID / 映像 /
//! 令牌都是测试进程，所以用 TrustPolicy 注入测试 exe 的文件名作为白名单（F1）。

use std::ffi::c_void;
use std::os::windows::io::AsRawHandle;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{
    GetTokenInformation, RevertToSelf, TokenImpersonationLevel, SECURITY_ATTRIBUTES, TOKEN_QUERY,
};
use windows_sys::Win32::System::Pipes::ImpersonateNamedPipeClient;
use windows_sys::Win32::System::Threading::{GetCurrentThread, OpenThreadToken};

use super::*;
use crate::mihomo::guard::PipeName;
use crate::mihomo::trust::{FactErr, Ident, LabelFact, PipeKind, TrustPolicy, Who};
use crate::win::util::{last_err, wide, LocalBox};

static SEQ: AtomicU32 = AtomicU32::new(0);

fn unique_name() -> String {
    format!(
        r"\\.\pipe\ca-m1-test-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    )
}

fn me() -> String {
    identity::known().unwrap().me_str.clone()
}

/// Verge sidecar 的 SDDL（本人 / SY / BA 全部权限），属主显式为本人。
fn verge_sddl() -> String {
    format!("O:{0}D:P(A;;GA;;;{0})(A;;GA;;;SY)(A;;GA;;;BA)", me())
}

fn test_policy() -> TrustPolicy {
    let exe = std::env::current_exe().unwrap();
    let mut p = TrustPolicy::production();
    p.images = vec![exe.file_name().unwrap().to_string_lossy().into_owned()];
    p
}

fn create_server(name: &str, sddl: &str, max_instances: usize) -> NamedPipeServer {
    let w = wide(sddl);
    let mut psd: *mut c_void = std::ptr::null_mut();
    // SAFETY: 以 NUL 结尾的 SDDL；psd 由 LocalBox 释放。
    let ok = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            w.as_ptr(),
            SDDL_REVISION_1,
            &mut psd,
            std::ptr::null_mut(),
        )
    };
    assert!(
        ok != 0,
        "SDDL {sddl}: {}",
        last_err("ConvertStringSecurityDescriptorToSecurityDescriptorW")
    );
    // SAFETY: 上面的 API 要求 LocalFree 释放。
    let sd = unsafe { LocalBox::from_raw(psd) };
    let mut sa = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: sd.as_ptr(),
        bInheritHandle: 0,
    };
    let sa_ptr = &mut sa as *mut SECURITY_ATTRIBUTES as usize;
    // tokio 要在运行时内注册句柄。
    in_rt(|| {
        // SAFETY: sa / sd 在调用期间有效（block_on 同步返回）。
        unsafe {
            ServerOptions::new()
                .first_pipe_instance(true)
                .max_instances(max_instances)
                .create_with_security_attributes_raw(name, sa_ptr as *mut c_void)
        }
        .expect("create pipe")
    })
}

/// 在 tauri 的 tokio 运行时上下文里同步执行（创建 / 打开管道需要 reactor）。
fn in_rt<T>(f: impl FnOnce() -> T) -> T {
    tauri::async_runtime::block_on(async move { f() })
}

/// 测试服务端实例的报告。`connected` 为真才说明 `bytes == 0` 有意义（Ellie PR50：
/// connect 超时同样会得到 0 字节，必须先证明该实例确实接受了连接）。跨用户回放（M-c）
/// 可复用 [`Report::assert_connected_zero_bytes`] / [`Report::summary_line`]。
#[derive(Debug, Default)]
pub(crate) struct Report {
    /// 该实例接受了一个客户端连接。
    pub(crate) connected: bool,
    /// 连接后收到的字节数。
    pub(crate) bytes: usize,
    /// 匿名级别检查：Ok 表示服务端无法拿到本进程身份。
    anonymous: Option<Result<(), String>>,
}

impl Report {
    /// 拒绝路径的断言：实例确实被连上，且收到 0 字节（F5）。
    pub(crate) fn assert_connected_zero_bytes(&self, what: &str) {
        assert!(
            self.connected,
            "{what}: 服务端实例从未被连接（0 字节不能说明问题）"
        );
        assert_eq!(self.bytes, 0, "{what}: 拒绝时服务端必须收到 0 字节（F5）");
    }
    /// 一行机器可读的摘要，供跨进程 / 跨用户的服务端打印、由父进程解析。
    #[allow(dead_code, reason = "供 M-c 跨用户回放的辅助进程使用")]
    pub(crate) fn summary_line(&self) -> String {
        format!(
            "PIPE_SERVER_REPORT connected={} bytes={}",
            self.connected, self.bytes
        )
    }
}

/// 服务端在模拟客户端后能否打开线程令牌；SECURITY_ANONYMOUS 下应失败（或级别为 Anonymous）。
fn anonymous_check(server: &NamedPipeServer) -> Result<(), String> {
    let h = server.as_raw_handle() as HANDLE;
    // SAFETY: 有效的管道句柄；同一线程内（无 await）模拟、检查、恢复。
    unsafe {
        if ImpersonateNamedPipeClient(h) == 0 {
            return Err(format!("{}", last_err("ImpersonateNamedPipeClient")));
        }
        let mut tok: HANDLE = std::ptr::null_mut();
        let opened = OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, 1, &mut tok) != 0;
        let open_err = last_err("OpenThreadToken");
        let mut verdict = Ok(());
        if opened {
            let mut level: i32 = -1;
            let mut len = 0u32;
            let got = GetTokenInformation(
                tok,
                TokenImpersonationLevel,
                &mut level as *mut i32 as *mut c_void,
                4,
                &mut len,
            ) != 0;
            CloseHandle(tok);
            if !got || level != 0 {
                verdict = Err(format!("服务端拿到了非匿名的模拟令牌（level {level}）"));
            }
        } else if open_err.code != 1347 {
            // ERROR_CANT_OPEN_ANONYMOUS
            verdict = Err(format!("意外的错误：{open_err}"));
        }
        assert!(RevertToSelf() != 0);
        verdict
    }
}

/// 接受一个连接；`respond` 时读完请求头、做匿名检查并回 200。返回是否连上与收到的字节数。
pub(crate) fn spawn_server(
    server: NamedPipeServer,
    respond: bool,
) -> tauri::async_runtime::JoinHandle<Report> {
    tauri::async_runtime::spawn(async move {
        let mut server = server;
        let mut rep = Report::default();
        if tokio::time::timeout(Duration::from_secs(10), server.connect())
            .await
            .is_err()
        {
            // connect 超时：connected = false，调用方据此判断 0 字节无意义。
            return rep;
        }
        rep.connected = true;
        let mut buf = vec![0u8; 8192];
        let mut got = Vec::new();
        loop {
            let n = match tokio::time::timeout(Duration::from_secs(3), server.read(&mut buf)).await
            {
                Ok(Ok(n)) => n,
                _ => 0,
            };
            if n == 0 {
                break;
            }
            got.extend_from_slice(&buf[..n]);
            if respond && got.windows(4).any(|w| w == b"\r\n\r\n") {
                rep.anonymous = Some(anonymous_check(&server));
                let _ = server
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                    )
                    .await;
                let _ = server.flush().await;
                break;
            }
        }
        rep.bytes = got.len();
        if respond {
            // 留出时间让客户端读完再断开。
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        // 不调用 DisconnectNamedPipe：它会丢弃客户端尚未读取的响应（客户端读到 233）。
        // 直接 drop 关闭句柄，客户端读完剩余数据后得到 EOF（BROKEN_PIPE），与 go-winio 一致。
        drop(server);
        rep
    })
}

fn request(name: &str, policy: &TrustPolicy) -> Result<(UnixHttpResult, PipeTrust), PipeErr> {
    let pn = PipeName::for_test(name.to_string());
    tauri::async_runtime::block_on(http_via_pipe_with(
        &pn,
        PipeKind::Sidecar,
        policy,
        "GET",
        "/version",
        None,
        3000,
    ))
}

fn finish(h: tauri::async_runtime::JoinHandle<Report>) -> Report {
    tauri::async_runtime::block_on(h).unwrap()
}

fn reject_reason(e: &PipeErr) -> Option<(&'static str, bool)> {
    match e {
        PipeErr::Untrusted(u) => Some((u.reason, false)),
        PipeErr::InsecureDacl(u) => Some((u.reason, true)),
        _ => None,
    }
}

fn assert_rejected_zero_bytes(sddl: &str, policy: &TrustPolicy, reason: &str, insecure_dacl: bool) {
    let name = unique_name();
    let srv = spawn_server(create_server(&name, sddl, 1), true);
    match request(&name, policy) {
        Err(e) => assert_eq!(
            reject_reason(&e),
            Some((reason, insecure_dacl)),
            "{sddl}: {e:?}"
        ),
        Ok((_, t)) => panic!("{sddl}: 应被拒绝，实际通过 {t:?}"),
    }
    finish(srv).assert_connected_zero_bytes(sddl);
}

#[test]
fn trusted_pipe_roundtrip_without_authorization_and_anonymous_qos() {
    let name = unique_name();
    let srv = spawn_server(create_server(&name, &verge_sddl(), 1), true);
    let (res, trust) = request(&name, &test_policy()).expect("verge SDDL 应通过");
    assert_eq!(res.status, 200);
    assert_eq!(res.body, "ok");
    assert_eq!(trust.owner.who, Who::Me);
    let rep = finish(srv);
    assert!(rep.connected && rep.bytes > 0);
    rep.anonymous
        .expect("anonymous check ran")
        .expect("SECURITY_ANONYMOUS");
}

/// F3′：真实管道上的 DACL 判定（V5 复跑 S-D′ + Ellie 复核 §2-1/§2-2）。
#[test]
fn f3_prime_real_pipe_dacls() {
    let me = me();
    let p = test_policy();
    for sddl in [
        // mihomo 默认 SDDL（BU 0x12019f 含 FILE_CREATE_PIPE_INSTANCE）
        "D:P(A;OICI;0x12019f;;;BU)(A;OICI;0x12019f;;;SY)".to_string(),
        format!("O:{me}D:P(A;;GA;;;{me})(A;;GA;;;SY)(A;;GA;;;BA)(A;;GA;;;WD)"),
        format!("O:{me}D:P(A;;GA;;;{me})(A;;GA;;;SY)(A;;GW;;;AU)"),
        format!("O:{me}D:P(A;;GA;;;{me})(A;;0x4;;;IU)"),
        format!("O:{me}D:P(A;;GA;;;{me})(A;;WD;;;WD)"),
    ] {
        assert_rejected_zero_bytes(&sddl, &p, "dacl-allows-foreign-instance", true);
    }
    // NULL DACL
    assert_rejected_zero_bytes(&format!("O:{me}D:NO_ACCESS_CONTROL"), &p, "dacl-null", true);

    // CI 实测（windows-2022）：写进管道 SD 的 MAXIMUM_ALLOWED 位不会保留（内核只在访问请求里认它），
    // 读回来的 DACL 不含危险位，因此判定通过。单测里仍覆盖「若读到 MAXIMUM_ALLOWED 就拒绝」。
    let name = unique_name();
    let sddl = format!("O:{me}D:P(A;;GA;;;{me})(A;;0x2000000;;;BU)");
    let srv = spawn_server(create_server(&name, &sddl, 1), true);
    assert!(request(&name, &p).is_ok(), "{sddl}");
    finish(srv);
}

/// cvsi 风格 AU 0x12019b（读写数据但不含 0x4）：通过，只带 S-2 提示（Ellie §2-4）。
#[test]
fn cvsi_style_au_without_create_instance_passes_with_hint() {
    let me = me();
    let name = unique_name();
    let sddl = format!("O:{me}D:P(A;;GA;;;{me})(A;;GA;;;SY)(A;;0x12019b;;;AU)");
    let srv = spawn_server(create_server(&name, &sddl, 1), true);
    let (res, trust) = request(&name, &test_policy()).expect("AU 0x12019b 应通过");
    assert_eq!(res.status, 200);
    assert!(
        trust.warnings.iter().any(|w| w.contains("AU=0x0012019B")),
        "{:?}",
        trust.warnings
    );
    finish(srv);
}

/// 空 DACL：判定逻辑放行，但真实管道上属主也没有读写数据的权限——打开即 ACCESS_DENIED，0 字节。
#[test]
fn empty_dacl_real_pipe_is_denied_at_open_with_zero_bytes() {
    let name = unique_name();
    let srv = spawn_server(create_server(&name, &format!("O:{}D:P", me()), 1), true);
    let r = request(&name, &test_policy());
    assert!(matches!(r, Err(PipeErr::Denied)), "{:?}", r.map(|(_, t)| t));
    // 打开就失败：实例从未被连上（这里 connected = false 正是要断言的）。
    let rep = finish(srv);
    assert!(!rep.connected && rep.bytes == 0, "{rep:?}");
}

/// 完整性标签低于 Medium ⇒ 拒绝（Ellie 建议项）。
#[test]
fn low_integrity_label_is_rejected_with_zero_bytes() {
    let me = me();
    let sddl = format!("O:{me}D:P(A;;GA;;;{me})(A;;GA;;;SY)S:(ML;;NW;;;LW)");
    assert_rejected_zero_bytes(&sddl, &test_policy(), "label-below-medium", true);
}

/// Ellie PR50 Q4：Medium 标签但不带 NO_WRITE_UP（`S:(ML;;NR;;;ME)`）⇒ 拒绝。
#[test]
fn medium_label_without_no_write_up_is_rejected_with_zero_bytes() {
    let me = me();
    let sddl = format!("O:{me}D:P(A;;GA;;;{me})(A;;GA;;;SY)S:(ML;;NR;;;ME)");
    assert_rejected_zero_bytes(&sddl, &test_policy(), "label-no-write-up", true);
}

/// S-D′ 进程内重放（同一用户）：mihomo 默认 SDDL 的管道上再挂第二个实例。实例共享 FCB 的
/// 属主 / DACL，所以无论客户端落到哪个实例都应被拒绝，两个实例都收到 0 字节。
/// 跨用户版本（真正的「外人」）见 PR 描述中的 TODO。
#[test]
fn s_d_prime_second_instance_on_mihomo_default_pipe_gets_zero_bytes() {
    let name = unique_name();
    let first = create_server(&name, "D:P(A;OICI;0x12019f;;;BU)(A;OICI;0x12019f;;;SY)", 4);
    let second = in_rt(|| {
        ServerOptions::new()
            .max_instances(4)
            .create(&name)
            .expect("second instance")
    });
    let a = spawn_server(first, true);
    let b = spawn_server(second, true);
    for _ in 0..2 {
        match request(&name, &test_policy()) {
            Err(PipeErr::InsecureDacl(u)) => assert_eq!(u.reason, "dacl-allows-foreign-instance"),
            other => panic!("应为 InsecureDacl，实际 {:?}", other.map(|(_, t)| t)),
        }
    }
    // 两个实例都必须确实被连上过，0 字节才有意义（Ellie PR50）。
    finish(a).assert_connected_zero_bytes("第一个实例");
    finish(b).assert_connected_zero_bytes("第二个实例");
}

/// 管道路径上映像 / L3 只是诊断（V5 S-D′、Ellie §8）：生产白名单不含测试 exe，也照样通过并告警。
#[test]
fn image_not_in_whitelist_is_warning_only() {
    let name = unique_name();
    let srv = spawn_server(create_server(&name, &verge_sddl(), 1), true);
    let (res, trust) = request(&name, &TrustPolicy::production()).expect("映像只作诊断");
    assert_eq!(res.status, 200);
    assert!(
        trust.warnings.iter().any(|w| w.starts_with("诊断")),
        "{:?}",
        trust.warnings
    );
    finish(srv);
}

#[test]
fn injected_owner_label_and_unreadable_sd_reject_with_zero_bytes() {
    let other = Ident::new(Who::Other, "S-1-5-21-9-9-9-1002");
    let mut p = test_policy();
    p.overrides.owner = Some(other.clone());
    assert_rejected_zero_bytes(&verge_sddl(), &p, "owner-mismatch", false);

    let mut p = test_policy();
    p.overrides.security_err = Some(FactErr::new("GetSecurityInfo", 5));
    assert_rejected_zero_bytes(&verge_sddl(), &p, "security-unreadable", false);

    let mut p = test_policy();
    p.overrides.label = Some(LabelFact::Rid(0x1000));
    assert_rejected_zero_bytes(&verge_sddl(), &p, "label-below-medium", true);

    // L3 为他人：只告警
    let mut p = test_policy();
    p.overrides.l3 = Some(Ok(other));
    let name = unique_name();
    let srv = spawn_server(create_server(&name, &verge_sddl(), 1), true);
    let (_, t) = request(&name, &p).expect("L3 只作诊断");
    assert!(!t.warnings.is_empty());
    finish(srv);
}

#[test]
fn busy_pipe_retries_until_deadline_then_busy() {
    let name = unique_name();
    let server = create_server(&name, &verge_sddl(), 1);
    // 占住唯一的实例。
    let holder = in_rt(|| {
        tokio::net::windows::named_pipe::ClientOptions::new()
            .open(&name)
            .expect("first client")
    });
    let pn = PipeName::for_test(name.clone());
    let started = Instant::now();
    let r = tauri::async_runtime::block_on(open_verified(
        &pn,
        PipeKind::Sidecar,
        &test_policy(),
        started + Duration::from_millis(300),
    ));
    assert!(matches!(r, Err(PipeErr::Busy)), "{:?}", r.map(|(_, t)| t));
    assert!(started.elapsed() >= Duration::from_millis(250));
    drop(holder);
    drop(server);
}

#[test]
fn missing_pipe_is_not_found() {
    let pn = PipeName::for_test(unique_name());
    let r = tauri::async_runtime::block_on(open_verified(
        &pn,
        PipeKind::Sidecar,
        &test_policy(),
        Instant::now() + Duration::from_millis(200),
    ));
    assert!(
        matches!(r, Err(PipeErr::NotFound)),
        "{:?}",
        r.map(|(_, t)| t)
    );
}
