//! 命名管道控制器传输（windows-plan r4 §2.2，M1）。
//!
//! 顺序固定：
//! 1. `ClientOptions::security_qos_flags(SECURITY_ANONYMOUS)` 打开（服务端无法以本进程身份
//!    做任何事；V3 / V5 P1）；`ERROR_PIPE_BUSY` 每 50 ms 重试直到截止时间；
//! 2. 在**同一个句柄**上用一次 `GetSecurityInfo(OWNER | DACL | LABEL)` 读安全描述符，交给纯函数
//!    `trust::decide_pipe` 判定（L1 属主 + F3′ DACL 白名单 + 完整性标签）；服务端 PID、L2、L3
//!    只作诊断（V5 S-D′：它们不能标识实际连上的实例）；
//! 3. 只有判定通过才写第一个字节（F5）；任何拒绝都直接关闭句柄，零字节。
//!
//! 管道上不发送 `Authorization`：mihomo 的管道 / Unix 套接字控制器不校验 secret，secret 不必
//! 离开本进程。

use std::os::windows::io::AsRawHandle;
use std::time::{Duration, Instant};

use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeClient};
use windows_sys::Win32::Storage::FileSystem::SECURITY_ANONYMOUS;

use super::guard::PipeName;
use super::trust::{tcp_fallback_after_pipes, PipeErr, PipeKind, PipeTrust, TrustPolicy};
use super::types::UnixHttpResult;
use crate::win::facts::OsPipeFacts;
use crate::win::identity;

const ERROR_FILE_NOT_FOUND: i32 = 2;
const ERROR_PATH_NOT_FOUND: i32 = 3;
const ERROR_ACCESS_DENIED: i32 = 5;
const ERROR_PIPE_BUSY: i32 = 231;

/// `ERROR_PIPE_BUSY` 的重试间隔（§2.2）。
pub const PIPE_BUSY_RETRY: Duration = Duration::from_millis(50);

/// 打开并校验。返回的客户端句柄**尚未写过任何字节**。
pub async fn open_verified(
    name: &PipeName,
    kind: PipeKind,
    policy: &TrustPolicy,
    deadline: Instant,
) -> Result<(NamedPipeClient, PipeTrust), PipeErr> {
    let known = identity::known().map_err(|e| PipeErr::Io(format!("读不到当前用户身份：{e}")))?;
    let client = loop {
        match ClientOptions::new()
            .security_qos_flags(SECURITY_ANONYMOUS)
            .open(name.raw())
        {
            Ok(c) => break c,
            Err(e) => match e.raw_os_error() {
                Some(ERROR_PIPE_BUSY) => {
                    let now = Instant::now();
                    if now >= deadline {
                        return Err(PipeErr::Busy);
                    }
                    tokio::time::sleep(PIPE_BUSY_RETRY.min(deadline - now)).await;
                }
                Some(ERROR_FILE_NOT_FOUND) | Some(ERROR_PATH_NOT_FOUND) => {
                    return Err(PipeErr::NotFound)
                }
                Some(ERROR_ACCESS_DENIED) => return Err(PipeErr::Denied),
                _ => return Err(PipeErr::Io(format!("open: {e}"))),
            },
        }
    };
    let trust = {
        let mut facts = OsPipeFacts::new(client.as_raw_handle(), known);
        policy.decide_pipe(kind, &mut facts)
    };
    match trust {
        Ok(t) => {
            for w in &t.warnings {
                crate::append_app_log(&format!("[pipe] {:?} 警告：{w}", name));
            }
            Ok((client, t))
        }
        Err(u) => {
            crate::append_app_log(&format!("[pipe] {:?} 拒绝：{u}", name));
            drop(client);
            Err(PipeErr::from_reject(u))
        }
    }
}

/// 打开、校验，然后发一个 HTTP 请求（不带 Authorization）。
pub async fn http_via_pipe_with(
    name: &PipeName,
    kind: PipeKind,
    policy: &TrustPolicy,
    method: &str,
    path: &str,
    body: Option<&str>,
    timeout_ms: u64,
) -> Result<(UnixHttpResult, PipeTrust), PipeErr> {
    // 先校验请求本身，再碰管道。
    super::transport::check_raw_request(method, path).map_err(PipeErr::Io)?;
    let started = Instant::now();
    let budget = Duration::from_millis(timeout_ms);
    let (client, trust) = open_verified(name, kind, policy, started + budget).await?;
    let left = budget
        .saturating_sub(started.elapsed())
        .max(Duration::from_millis(1));
    let res = super::transport::http_over_stream(
        client,
        method,
        path,
        body,
        None,
        left.as_millis() as u64,
    )
    .await
    .map_err(PipeErr::Io)?;
    Ok((res, trust))
}

/// 生产入口（同步，供 `spawn_blocking` 中的调用方使用）：生产 TrustPolicy，失败时把结果
/// **收紧**进 M-3 回落状态（Ellie PR50 M-a：单次请求的成功 / NotFound 不放宽缓存）。
pub fn http_via_pipe_blocking(
    name: &PipeName,
    kind: PipeKind,
    method: &str,
    path: &str,
    body: Option<&str>,
    timeout_ms: u64,
) -> Result<(UnixHttpResult, PipeTrust), PipeErr> {
    let policy = TrustPolicy::production();
    let r = tauri::async_runtime::block_on(http_via_pipe_with(
        name, kind, &policy, method, path, body, timeout_ms,
    ));
    if let Err(e) = &r {
        super::win_tcp::tighten_pipe_outcome(tcp_fallback_after_pipes(std::slice::from_ref(e)));
    }
    r
}

#[cfg(test)]
mod tests;
