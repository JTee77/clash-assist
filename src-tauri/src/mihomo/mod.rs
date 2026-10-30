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
// 非 Unix 上暂无调用方（Unix 套接字传输不编译）；M1 的命名管道传输复用这里的解析。
#[cfg_attr(not(unix), allow(dead_code))]
mod http_parse;
mod nodes;
mod timed;
mod transport;
mod types;

// Public controller-layer API surface (referenced as `mihomo::X` from `crate::lib`).
pub use discovery::{discover_controller, discover_for_client, read_verge_config};
pub use nodes::list_nodes_async;
pub use cancel::{arm_cancel, disarm_cancel, fire_cancel};
pub use timed::proxy_timed_transfer_async;
pub use transport::{http_via_tcp_async, http_via_unix, proxy_fetch_async};
pub use types::{DiscoverResult, ListNodesResult, TimedTransferResult, UnixHttpResult};

