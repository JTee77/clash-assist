//! Shared data types + constants for the mihomo controller layer.
//! These are the plain structs returned to the frontend and the tuning knobs
//! (default ports, body caps, proxy-type/name filters) reused across transport,
//! discovery and nodes modules.

use serde::Serialize;

pub const DEFAULT_PORT: u16 = 9097;
pub const DEFAULT_MIXED: u16 = 7897;

/// Cap /proxies (and similar) IPC bodies so huge delay-history payloads cannot kill the webview.
pub const MAX_BODY_BYTES: usize = 6 * 1024 * 1024;

/// Cap for bandwidth-sample transfers (separate from controller IPC cap).
pub(crate) const MAX_TIMED_BODY_BYTES: usize = 8 * 1024 * 1024;

/// Proxy group types that are not real leaf nodes.
pub(crate) const IGNORE_PROXY_TYPES: &[&str] = &[
    "Selector",
    "URLTest",
    "Fallback",
    "LoadBalance",
    "Relay",
    "Direct",
    "Reject",
    "Compatible",
    "Pass",
];

/// Node-name substrings that indicate marketing/junk entries rather than a real proxy.
pub(crate) const JUNK_NAME_KEYWORDS: &[&str] = &["剩余", "到期", "官网"];

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoverResult {
    pub host: String,
    pub port: u16,
    pub secret: String,
    pub mixed_port: u16,
    pub source: String,
    pub sock_path: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnixHttpResult {
    pub status: u16,
    pub body: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SlimNode {
    pub name: String,
    #[serde(rename = "type")]
    pub node_type: String,
    /// Capability flags from mihomo `/proxies`. Each is a real, independent
    /// per-node boolean (absent → false). The full vocabulary mihomo emits is
    /// these six; whether a given subscription lights any of them depends on
    /// its node configs, so none is assumed always-on.
    /// Forwards UDP traffic.
    pub udp: bool,
    /// XUDP transport.
    pub xudp: bool,
    /// UDP over TCP.
    pub uot: bool,
    /// TCP Fast Open.
    pub tfo: bool,
    /// Multiplexing (smux).
    pub smux: bool,
    /// Multipath TCP.
    pub mptcp: bool,
    /// Client-reported health: mihomo `alive` flag. None = field absent.
    pub alive: Option<bool>,
    /// Last delay-test result from the client (`history` last entry, ms).
    /// 0 = the client's most recent test failed. None = client never tested.
    pub last_delay: Option<u32>,
    /// ISO-8601 time of that last delay test (freshness check for skip logic).
    pub last_delay_at: Option<String>,
}

/// One policy group, as needed by the frontend's single current-proxy resolver.
///
/// `all` is shipped verbatim (not reduced to a count) on purpose: counting real
/// leaves requires the leaf predicate, and keeping that predicate in one place
/// (TS `currentProxy.ts`) is what stops Rust and the webview from disagreeing
/// about which node is "in use". Measured cost on a real 17-group / 34-node
/// subscription: +13 KB per `mihomo_list_nodes` call (6.9 KB → 20 KB).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SlimGroup {
    pub name: String,
    #[serde(rename = "type")]
    pub group_type: String,
    pub now: Option<String>,
    pub all: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListNodesResult {
    pub nodes: Vec<SlimNode>,
    /// Every Selector / URLTest / Fallback / LoadBalance group. The frontend
    /// derives the current node from this + `nodes`; Rust no longer resolves it.
    pub groups: Vec<SlimGroup>,
    pub status: u16,
    pub error: Option<String>,
    pub unauthorized: bool,
    /// Which transport produced this result: "tcp" | "unix"
    pub transport: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TimedTransferResult {
    pub ok: bool,
    pub status: u16,
    pub bytes: u64,
    pub elapsed_ms: u64,
    pub error: Option<String>,
}
