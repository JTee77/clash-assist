//! Clash Verge Rev / Mihomo controller + HTTP transport, split into cohesive
//! submodules: `types` (shared data + constants), `discovery` (controller/client
//! path resolution), `http_parse` (raw response/chunked decoding), `transport`
//! (TCP + Unix-socket HTTP), `timed` (bandwidth probe), and `nodes` (`/proxies`
//! slim + list). This file is a bucket that re-exports the public surface used
//! by `crate::lib` (Tauri commands).
//! Secrets are returned to the frontend for local API use only; never log secret values.
//! OS-specific paths/sockets live in `crate::platform` (macOS behavior unchanged).

mod cancel;
mod discovery;
pub mod guard;
mod http_parse;
mod nodes;
// Windows 命名管道传输（M1，§2.2）。
#[cfg(windows)]
mod pipe;
mod timed;
mod transport;
// §2.2 / §2.1 规则 4 的纯判定逻辑：Windows 使用，全平台单测。
#[cfg(any(windows, test))]
pub(crate) mod trust;
mod types;
// Windows TCP 控制器出口（规则 4 监听者校验、secret 闸门、M-3）。
#[cfg(windows)]
mod win_tcp;

// Public controller-layer API surface (referenced as `mihomo::X` from `crate::lib`).
pub use cancel::{arm_cancel, disarm_cancel, fire_cancel};
#[cfg(not(windows))]
pub use discovery::read_verge_config;
#[cfg(windows)]
pub use discovery::read_verge_config_redacted;
pub use discovery::{discover_controller, discover_for_client};
pub use nodes::list_nodes_async;
pub use timed::proxy_timed_transfer_async;
pub use transport::{http_via_tcp_async, http_via_unix, proxy_fetch_async};
pub use types::{DiscoverResult, ListNodesResult, TimedTransferResult, UnixHttpResult};
