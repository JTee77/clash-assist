//! Windows 控制器 TCP 出口（windows-plan r4 §2.1 规则 4、M-3，M1）。
//!
//! 一次请求的顺序：
//! 1. M-3：命名管道候选里只要有一个被判为不可信（或出现其他错误），就**不回落 TCP**；
//! 2. 规则 4：端口 P 的监听表 → 唯一监听进程 → WTS 快速拒绝 → L2 ① 白名单 → L3 = 本人
//!    ⇒ `listener-verified`；其余 ⇒ `listener-unverified`（M1 策略：拒绝，不建连）或拒绝；
//! 3. secret：只有 `listener-verified` **且** [`WINDOWS_TCP_SECRET_GATE_OPEN`] 打开才发 Bearer。
//!    闸门在 M1 恒为关闭——有 secret 时直接拒绝、不建连（M0 fail-closed 延续，等待 V5 复跑
//!    S-F2 / S-D′ 结论写入 PR，Ellie 时序条件）；
//! 4. 建连后按 (本端, 对端) 在连接表里找到服务端那一行，PID 必须等于校验通过的 PID（Ellie S1），
//!    通过之后才写第一个字节。L2 ① / L3 用过的进程句柄一直持有到这一步完成（Ellie PR50 Q3），
//!    防止校验与建连之间 PID 被复用。

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::trust::{tcp_send_plan, ListenerCheck, TcpFallback, TcpPlan, TrustPolicy};
use super::types::UnixHttpResult;
use crate::win::facts::OsProcessFacts;
use crate::win::process::ProcessProbe;
use crate::win::{identity, tcp};

/// **secret 经 TCP 发送的闸门。M1 恒为 `false`。**
///
/// 打开条件（Ellie 时序条件 d）：V5 复跑（S-F2 / S-D′）结论写入 PR 并经评审同意。
/// 判定逻辑（`trust::tcp_send_plan`）已完整实现并测试；这里只是开关。
///
/// 开闸前置条件（Ellie PR50 Q3，已实现）：`http_via_tcp_with` 持有监听进程句柄直到 post-connect
/// 校验完成；拿不到句柄时拒绝发送。改动这条路径时不得回退该约束。
pub const WINDOWS_TCP_SECRET_GATE_OPEN: bool = false;

/// M-3 管道结果的有效期：过期后 TCP 请求会重新探测管道候选。
const PIPE_OUTCOME_TTL: Duration = Duration::from_secs(30);

/// M-3 回落状态缓存。Ellie PR50 M-a：只有完整的候选探测能覆盖（包括放宽）；单次管道请求
/// 只能收紧（见 [`trust::tighten_fallback`](super::trust::tighten_fallback)）。
#[derive(Debug, Default)]
pub(crate) struct PipeOutcomeCache {
    slot: Option<(Instant, TcpFallback)>,
}

impl PipeOutcomeCache {
    pub(crate) const fn new() -> Self {
        PipeOutcomeCache { slot: None }
    }
    fn valid(&self, now: Instant) -> Option<&TcpFallback> {
        match &self.slot {
            Some((at, f)) if now.saturating_duration_since(*at) < PIPE_OUTCOME_TTL => Some(f),
            _ => None,
        }
    }
    /// 完整探测的结论：直接覆盖。
    pub(crate) fn record_probe(&mut self, f: TcpFallback, now: Instant) {
        self.slot = Some((now, f));
    }
    /// 单次请求的结论：只在更严格时写入。
    pub(crate) fn tighten(&mut self, f: TcpFallback, now: Instant) {
        if let Some(f) = super::trust::tighten_fallback(self.valid(now), f) {
            self.slot = Some((now, f));
        }
    }
    pub(crate) fn get(&self, now: Instant) -> Option<TcpFallback> {
        self.valid(now).cloned()
    }
}

static LAST_PIPE_OUTCOME: Mutex<PipeOutcomeCache> = Mutex::new(PipeOutcomeCache::new());

/// 一轮完整管道候选探测（discovery）的 M-3 结论：覆盖缓存。
pub fn record_probe_outcome(f: TcpFallback) {
    if let Ok(mut g) = LAST_PIPE_OUTCOME.lock() {
        g.record_probe(f, Instant::now());
    }
}

/// 单次管道请求的 M-3 结论：只能收紧（成功 / NotFound 不会把 Blocked 放宽为 Allowed）。
pub fn tighten_pipe_outcome(f: TcpFallback) {
    if let Ok(mut g) = LAST_PIPE_OUTCOME.lock() {
        g.tighten(f, Instant::now());
    }
}

fn cached_pipe_outcome() -> Option<TcpFallback> {
    LAST_PIPE_OUTCOME.lock().ok()?.get(Instant::now())
}

/// 当前的 M-3 回落状态：缓存有效则用缓存，否则重新探测管道候选。
async fn fallback_state() -> TcpFallback {
    if let Some(f) = cached_pipe_outcome() {
        return f;
    }
    match tauri::async_runtime::spawn_blocking(super::discovery::windows_probe_verge_pipes).await {
        Ok(p) => p.fallback,
        Err(e) => TcpFallback::Blocked(format!("管道探测任务失败，不回落 TCP：{e}")),
    }
}

/// `localhost` 直接映射到 127.0.0.1（不做 DNS）；其余必须是回环 IP 字面量。
pub(crate) fn loopback_addr(host: &str, port: u16) -> Result<SocketAddr, String> {
    let h = host.trim();
    let ip: IpAddr = if h.eq_ignore_ascii_case("localhost") {
        IpAddr::V4(Ipv4Addr::LOCALHOST)
    } else {
        h.trim_start_matches('[')
            .trim_end_matches(']')
            .parse()
            .map_err(|_| format!("控制器地址必须是回环 IP：{host:?}"))?
    };
    if !ip.is_loopback() {
        return Err(format!("控制器地址必须是回环 IP：{host:?}"));
    }
    Ok(SocketAddr::new(ip, port))
}

/// 规则 4 的监听者校验（同步，FFI）。
pub fn check_listener(policy: &TrustPolicy, addr: SocketAddr) -> ListenerCheck {
    check_listener_held(policy, addr).0
}

/// 同 [`check_listener`]，并在 Verified 时交出 L2 ① / L3 用过的进程句柄（Ellie PR50 Q3）。
fn check_listener_held(
    policy: &TrustPolicy,
    addr: SocketAddr,
) -> (ListenerCheck, Option<ProcessProbe>) {
    let known = match identity::known() {
        Ok(k) => k,
        Err(e) => {
            let c = ListenerCheck::Unverified {
                pid: None,
                reasons: vec![format!("读不到当前用户身份：{e}")],
                service_mode: false,
            };
            return (c, None);
        }
    };
    let v4 = tcp::listeners_v4(addr.port());
    let v6 = tcp::listeners_v6(addr.port());
    let mut facts = OsProcessFacts::new(known);
    let check = policy.decide_tcp(addr, v4, v6, &mut facts);
    let held = match &check {
        ListenerCheck::Verified { pid, .. } => facts.into_held(*pid),
        _ => None,
    };
    (check, held)
}

/// 生产入口（`transport::http_via_tcp_async` 在 Windows 上的实现）。
///
/// 有效 secret：前端传入的非空 secret，否则 Verge yaml 里的 secret（发现阶段在 Windows 上
/// 永远不把 secret 交给前端，由 Rust 自己取用）。
pub async fn http_via_tcp(
    host: &str,
    port: u16,
    method: &str,
    path: &str,
    body: Option<&str>,
    secret: &str,
    timeout_ms: u64,
) -> Result<UnixHttpResult, String> {
    super::transport::check_raw_request(method, path)?;
    let addr = loopback_addr(host, port)?;
    let fallback = fallback_state().await;
    let effective = if secret.is_empty() {
        tauri::async_runtime::spawn_blocking(super::discovery::windows_yaml_secret)
            .await
            .ok()
            .flatten()
    } else {
        Some(secret.to_string())
    };
    let policy = TrustPolicy::production();
    http_via_tcp_with(
        &policy,
        WINDOWS_TCP_SECRET_GATE_OPEN,
        fallback,
        addr,
        method,
        path,
        body,
        effective.as_deref(),
        timeout_ms,
    )
    .await
    .map(|(r, _)| r)
}

/// 可注入版本（测试用 `gate_open = true` 覆盖 SendBearer 分支；生产恒传闸门常量）。
/// 返回值附带来源标签 `verge-tcp(listener-verified)`。
#[allow(clippy::too_many_arguments)]
pub(crate) async fn http_via_tcp_with(
    policy: &TrustPolicy,
    gate_open: bool,
    fallback: TcpFallback,
    addr: SocketAddr,
    method: &str,
    path: &str,
    body: Option<&str>,
    secret: Option<&str>,
    timeout_ms: u64,
) -> Result<(UnixHttpResult, &'static str), String> {
    super::transport::check_raw_request(method, path)?;
    #[cfg(test)]
    let fallback = policy.overrides.fallback.clone().unwrap_or(fallback);

    // `held`：一直存活到函数结束（post-connect 校验与整个请求之后才关闭句柄）。
    let (check, held) = check_listener_held(policy, addr);
    let has_secret = secret.is_some_and(|s| !s.is_empty());
    let plan = tcp_send_plan(&check, has_secret, gate_open, &fallback);
    let (pid, label) = match (&plan, &check) {
        (TcpPlan::Refuse(why), _) => {
            crate::append_app_log(&format!("[tcp] {addr} 拒绝：{why}"));
            return Err(why.clone());
        }
        (_, ListenerCheck::Verified { pid, .. }) => {
            if held.is_none() {
                let why = format!("监听进程 {pid} 的句柄未能保持到建连之后（防 PID 复用），拒绝");
                crate::append_app_log(&format!("[tcp] {addr} 拒绝：{why}"));
                return Err(why);
            }
            (*pid, "verge-tcp(listener-verified)")
        }
        // tcp_send_plan 只对 Verified 返回发送计划。
        _ => return Err("内部错误：未验证的监听者不应进入发送路径".to_string()),
    };

    let budget = Duration::from_millis(timeout_ms);
    let started = Instant::now();
    let stream = tokio::time::timeout(budget, tokio::net::TcpStream::connect(addr))
        .await
        .map_err(|_| format!("connect {addr}: timed out"))?
        .map_err(|e| format!("connect {addr}: {e}"))?;
    // Ellie S1：连接后绑定。失败则直接关闭连接，零字节。
    let local = stream
        .local_addr()
        .map_err(|e| format!("local_addr: {e}"))?;
    let peer = stream.peer_addr().map_err(|e| format!("peer_addr: {e}"))?;
    let observed = tcp::connection_server_pid(peer, local);
    if let Err(why) = policy.post_connect(pid, observed) {
        drop(stream);
        crate::append_app_log(&format!("[tcp] {addr} 连接后校验失败：{why}"));
        return Err(why);
    }
    let bearer = match plan {
        TcpPlan::SendBearer => secret,
        _ => None,
    };
    let left = budget
        .saturating_sub(started.elapsed())
        .max(Duration::from_millis(1));
    let res = super::transport::http_over_stream(
        stream,
        method,
        path,
        body,
        bearer,
        left.as_millis() as u64,
    )
    .await?;
    Ok((res, label))
}

#[cfg(test)]
mod tests;
