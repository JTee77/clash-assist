//! `/proxies` handling: slim the controller's proxy JSON down to real leaf
//! nodes (dropping selector/urltest groups, junk names; keeping only the
//! client's last delay result instead of full history arrays) and the
//! TCP→Unix fallback that fetches it for the node list.

use serde_json::Value;

use super::transport::{http_via_tcp_async, http_via_unix};
use super::types::{ListNodesResult, SlimNode, IGNORE_PROXY_TYPES, JUNK_NAME_KEYWORDS};

fn is_junk_name(name: &str) -> bool {
    if name.starts_with("PASS") || name.starts_with("REJECT") {
        return true;
    }
    JUNK_NAME_KEYWORDS.iter().any(|k| name.contains(k))
}

fn ignore_type(t: &str) -> bool {
    IGNORE_PROXY_TYPES.iter().any(|x| *x == t)
}

/// A per-node capability flag from a proxy object; absent → false.
fn read_bool(p: &Value, key: &str) -> bool {
    p.get(key).and_then(|v| v.as_bool()).unwrap_or(false)
}

/// Client-side health from a proxy object: `alive` flag (None when absent).
fn read_alive(p: &Value) -> Option<bool> {
    p.get("alive").and_then(|v| v.as_bool())
}

/// Client's most recent delay-test result: last `history` entry's delay (ms),
/// 0 = that test failed. Returns (delay, time) — time is the ISO-8601 stamp
/// of that test, needed to tell "failed just now" from "failed hours ago".
fn read_last_delay(p: &Value) -> (Option<u32>, Option<String>) {
    let Some(last) = p.get("history").and_then(|v| v.as_array()).and_then(|a| a.last())
    else {
        return (None, None);
    };
    let delay = last.get("delay").and_then(|d| d.as_u64());
    let time = last
        .get("time")
        .and_then(|t| t.as_str())
        .map(|s| s.to_string());
    let delay = delay.map(|d| u32::try_from(d).unwrap_or(u32::MAX));
    (delay, time)
}

/// Build a slim node from a proxy object, reading the full six-flag mihomo
/// capability vocabulary in one place. Adding a future flag only means touching
/// this function + the `SlimNode` struct — no call-site churn.
fn slim_node(name: String, node_type: String, p: &Value) -> SlimNode {
    let (last_delay, last_delay_at) = read_last_delay(p);
    SlimNode {
        name,
        node_type,
        udp: read_bool(p, "udp"),
        xudp: read_bool(p, "xudp"),
        uot: read_bool(p, "uot"),
        tfo: read_bool(p, "tfo"),
        smux: read_bool(p, "smux"),
        mptcp: read_bool(p, "mptcp"),
        alive: read_alive(p),
        last_delay,
        last_delay_at,
    }
}

/// Group types that route to a member via their `now` field.
const GROUP_TYPES: &[&str] = &["Selector", "URLTest", "Fallback", "LoadBalance"];

/// User-facing selector groups, most authoritative first. `GLOBAL` is the
/// global-mode pseudo-group and often sits at `DIRECT` while the client is in
/// rule mode, so it must not be trusted blindly — hence the ordered scan below
/// that only accepts a chain ending at a real leaf node.
const PREFER_GROUP_NAMES: &[&str] = &[
    "Proxy",
    "GLOBAL",
    "proxy",
    "SELECT",
    "节点选择",
    "手动选择",
    "自动选择",
];

const MAX_HOPS: usize = 8;

/// A name is a real leaf proxy iff it exists, is not a group/pseudo type
/// (Direct/Reject/Pass/Selector… — i.e. not DIRECT), and is not a junk entry.
fn is_real_leaf(proxies: &serde_json::Map<String, Value>, name: &str) -> bool {
    let Some(p) = proxies.get(name) else {
        return false;
    };
    let t = p.get("type").and_then(|x| x.as_str()).unwrap_or("");
    if ignore_type(t) {
        return false;
    }
    !is_junk_name(name)
}

/// 3 = main picker (节点选择 / proxy / …), 2 = some other「选择」group,
/// 1 = name mentions 节点 (often a region subgroup), 0 = neither.
fn name_rank(name: &str) -> u8 {
    let lower = name.to_lowercase();
    if name.contains("节点选择")
        || name.contains("手动选择")
        || name.contains("选择节点")
        || name.contains("代理选择")
        || name.contains("选择代理")
        || lower.contains("proxy")
    {
        3
    } else if name.contains("选择") {
        2
    } else if name.contains("节点") {
        1
    } else {
        0
    }
}

/// Walk `now` through nested groups until a real leaf. Cycles and DIRECT stop as None.
fn follow_to_leaf(proxies: &serde_json::Map<String, Value>, start: &str) -> Option<String> {
    let mut seen = std::collections::BTreeSet::<String>::new();
    let mut current = start.to_string();
    for _ in 0..MAX_HOPS {
        if !seen.insert(current.clone()) {
            return None;
        }
        if is_real_leaf(proxies, &current) {
            return Some(current);
        }
        let group = proxies.get(&current)?;
        let t = group.get("type").and_then(|x| x.as_str()).unwrap_or("");
        if !GROUP_TYPES.contains(&t) {
            return None;
        }
        current = group.get("now").and_then(|n| n.as_str())?.to_string();
    }
    None
}

fn real_leaf_count(proxies: &serde_json::Map<String, Value>, group: &Value) -> usize {
    group
        .get("all")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter(|item| item.as_str().is_some_and(|n| is_real_leaf(proxies, n)))
                .count()
        })
        .unwrap_or(0)
}

struct Cand {
    leaf: String,
    leaves: usize,
    rank: u8,
    selector: bool,
}

/// Highest first key, then second. Different leaves at that peak → None.
fn unique_by(cands: &[Cand], rank_first: bool) -> Option<String> {
    let key = |c: &Cand| -> (usize, usize) {
        if rank_first {
            (c.rank as usize, c.leaves)
        } else {
            (c.leaves, c.rank as usize)
        }
    };
    let best = cands.iter().map(key).max()?;
    let top: Vec<&Cand> = cands.iter().filter(|c| key(c) == best).collect();
    let leaf = &top[0].leaf;
    if top.iter().all(|c| &c.leaf == leaf) {
        Some(leaf.clone())
    } else {
        None
    }
}

/// Resolve the node the user is actually on. Never surfaces DIRECT/PASS/REJECT
/// or a group name.
///
/// Exact preferred names are followed through nested selector chains. When a
/// subscription renames the main picker (「🚀 节点选择」) and policy groups
/// disagree, that main-picker selector wins. Otherwise the selector with the
/// most real leaves wins. None when still ambiguous, or when the main picker
/// itself is on DIRECT.
pub fn resolve_current_proxy(proxies: &serde_json::Map<String, Value>) -> Option<String> {
    for g in PREFER_GROUP_NAMES {
        if proxies.contains_key(*g) {
            if let Some(n) = follow_to_leaf(proxies, g) {
                return Some(n);
            }
        }
    }

    let mut strong: Vec<Cand> = Vec::new();
    let mut saw_strong_selector = false;
    let mut resolved: Vec<Cand> = Vec::new();

    for (name, p) in proxies {
        let t = p.get("type").and_then(|x| x.as_str()).unwrap_or("");
        if !GROUP_TYPES.contains(&t) {
            continue;
        }
        let rank = name_rank(name);
        let selector = t == "Selector";
        if selector && rank >= 2 {
            saw_strong_selector = true;
        }
        let Some(leaf) = follow_to_leaf(proxies, name) else {
            continue;
        };
        let cand = Cand {
            leaf,
            leaves: real_leaf_count(proxies, p),
            rank,
            selector,
        };
        if selector && rank >= 2 {
            strong.push(Cand {
                leaf: cand.leaf.clone(),
                leaves: cand.leaves,
                rank: cand.rank,
                selector: true,
            });
        }
        resolved.push(cand);
    }

    if saw_strong_selector {
        return unique_by(&strong, true);
    }

    if resolved.is_empty() {
        return None;
    }
    let only = resolved[0].leaf.clone();
    if resolved.iter().all(|c| c.leaf == only) {
        return Some(only);
    }

    let selectors: Vec<Cand> = resolved
        .iter()
        .filter(|c| c.selector)
        .map(|c| Cand {
            leaf: c.leaf.clone(),
            leaves: c.leaves,
            rank: c.rank,
            selector: c.selector,
        })
        .collect();
    if selectors.is_empty() {
        unique_by(&resolved, false)
    } else {
        unique_by(&selectors, false)
    }
}

/// Parse /proxies JSON into a slim node list (no history arrays; only the
/// last delay result + alive flag are kept).
pub fn slim_nodes_from_proxies_json(body: &str) -> Result<ListNodesResult, String> {
    let root: Value = serde_json::from_str(body).map_err(|e| format!("json: {e}"))?;
    let proxies = root
        .get("proxies")
        .and_then(|v| v.as_object())
        .ok_or_else(|| "missing proxies object".to_string())?;

    let current_proxy = resolve_current_proxy(proxies);

    let mut nodes: Vec<SlimNode> = Vec::new();
    for (name, p) in proxies {
        let node_type = p
            .get("type")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string();
        if ignore_type(&node_type) || is_junk_name(name) {
            continue;
        }
        nodes.push(slim_node(name.clone(), node_type, p));
    }

    if nodes.is_empty() {
        // Resolve leaf names from Selector / URLTest / Fallback `all` arrays.
        let mut leaf_names = std::collections::BTreeSet::new();
        let consider_all = |all: Option<&Value>, set: &mut std::collections::BTreeSet<String>| {
            let Some(arr) = all.and_then(|v| v.as_array()) else {
                return;
            };
            for item in arr {
                let Some(name) = item.as_str() else { continue };
                let Some(p) = proxies.get(name) else { continue };
                let t = p.get("type").and_then(|x| x.as_str()).unwrap_or("");
                if ignore_type(t) || is_junk_name(name) {
                    continue;
                }
                set.insert(name.to_string());
            }
        };

        for g in ["Proxy", "GLOBAL", "proxy"] {
            if let Some(p) = proxies.get(g) {
                consider_all(p.get("all"), &mut leaf_names);
            }
        }
        for (_name, p) in proxies {
            let t = p.get("type").and_then(|x| x.as_str()).unwrap_or("");
            if matches!(t, "Selector" | "URLTest" | "Fallback") {
                consider_all(p.get("all"), &mut leaf_names);
            }
        }

        nodes = leaf_names
            .into_iter()
            .filter_map(|name| {
                let p = proxies.get(&name)?;
                let node_type = p
                    .get("type")
                    .and_then(|t| t.as_str())
                    .unwrap_or("")
                    .to_string();
                Some(slim_node(name, node_type, p))
            })
            .collect();
    }

    Ok(ListNodesResult {
        nodes,
        current_proxy,
        status: 200,
        error: None,
        unauthorized: false,
        transport: None,
    })
}


/// Only supplement members actually referenced by controller groups. Provider names
/// are not a global node identity; never overwrite a top-level proxy or pick an
/// arbitrary same-name entry from different subscriptions.
fn missing_provider_names(root: &Value) -> Result<std::collections::BTreeSet<String>, String> {
    let proxies = root.get("proxies").and_then(Value::as_object)
        .ok_or_else(|| "missing proxies object".to_string())?;
    let mut missing = std::collections::BTreeSet::new();
    for group in proxies.values() {
        if !GROUP_TYPES.contains(&group.get("type").and_then(Value::as_str).unwrap_or("")) { continue; }
        let names = group.get("all").and_then(Value::as_array).into_iter().flatten()
            .filter_map(Value::as_str)
            .chain(group.get("now").and_then(Value::as_str));
        for name in names {
            if !name.is_empty() && !is_junk_name(name) && !proxies.contains_key(name) {
                missing.insert(name.to_string());
            }
        }
    }
    Ok(missing)
}

fn merge_provider_nodes(root: &mut Value, provider_root: &Value) -> Result<(), String> {
    let providers = provider_root.get("providers").and_then(Value::as_object)
        .ok_or_else(|| "missing providers object".to_string())?;
    for name in missing_provider_names(root)? {
        let mut subscriptions = Vec::new();
        let mut compatible = Vec::new();
        for provider in providers.values() {
            let Some(nodes) = provider.get("proxies").and_then(Value::as_array) else { continue; };
            for node in nodes {
                let node_type = node.get("type").and_then(Value::as_str).unwrap_or("");
                if node.get("name").and_then(Value::as_str) != Some(name.as_str())
                    || node_type.is_empty() || ignore_type(node_type) { continue; }
                if provider.get("vehicleType").and_then(Value::as_str) == Some("Compatible") {
                    compatible.push(node);
                } else { subscriptions.push(node); }
            }
        }
        let candidates = if subscriptions.is_empty() { compatible } else { subscriptions };
        if candidates.len() == 1 {
            root.get_mut("proxies").and_then(Value::as_object_mut).unwrap()
                .insert(name, candidates[0].clone());
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum NodeTransport<'a> { Tcp(&'a str, u16), Unix(&'a str) }

/// Read provider metadata on the SAME controller/transport as /proxies. No
/// subscription updates, healthchecks or selector writes occur during discovery.
async fn parse_nodes_with_providers(
    body: &str, secret: &str, timeout_ms: u64, transport: NodeTransport<'_>,
) -> Result<ListNodesResult, String> {
    let mut root: Value = serde_json::from_str(body).map_err(|e| format!("json: {e}"))?;
    let mut provider_error = None;
    if !missing_provider_names(&root)?.is_empty() {
        let response = match transport {
            NodeTransport::Tcp(host, port) => http_via_tcp_async(host, port, "GET", "/providers/proxies", None, secret, timeout_ms).await,
            NodeTransport::Unix(sock) => {
                let (sock, secret) = (sock.to_string(), secret.to_string());
                tauri::async_runtime::spawn_blocking(move || http_via_unix("GET", "/providers/proxies", None, &secret, Some(&sock), timeout_ms))
                    .await.map_err(|e| format!("provider task join: {e}"))?
            }
        };
        match response {
            Ok(response) if (200..300).contains(&response.status) => {
                match serde_json::from_str::<Value>(&response.body)
                    .map_err(|_| "订阅节点响应无法解析".to_string())
                    .and_then(|providers| merge_provider_nodes(&mut root, &providers)) {
                    Ok(()) => {},
                    Err(_) => provider_error = Some("订阅节点响应无法解析".to_string()),
                }
            }
            Ok(response) => provider_error = Some(format!("订阅节点接口返回 HTTP {}", response.status)),
            Err(_) => provider_error = Some("暂时读不到订阅节点接口".to_string()),
        }
    }
    let mut result = slim_nodes_from_proxies_json(&root.to_string())?;
    let missing = missing_provider_names(&root)?.len();
    if missing > 0 {
        let detail = provider_error.unwrap_or_else(|| "订阅中有同名、缺失或无效节点，无法唯一识别".to_string());
        result.error = Some(format!("已连上软件，但有 {missing} 个订阅节点未能读取（{detail}）"));
    } else if result.nodes.is_empty() {
        result.error = Some("已连接但未解析到可用节点，请到设置检查 Secret / 刷新，或确认订阅已加载".to_string());
    }
    Ok(result)
}

async fn try_list_nodes_unix(
    secret: &str,
    sock_path: Option<&str>,
    unix_timeout: u64,
) -> Result<Option<ListNodesResult>, String> {
    let Some(sock) = sock_path.map(|s| s.to_string()).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    let secret_owned = secret.to_string();
    let unix_attempt = tauri::async_runtime::spawn_blocking(move || {
        http_via_unix(
            "GET",
            "/proxies",
            None,
            &secret_owned,
            Some(sock.as_str()),
            unix_timeout,
        )
    })
    .await
    .map_err(|e| format!("unix task join: {e}"))?;

    match unix_attempt {
        Ok(res) if res.status == 401 || res.status == 403 => Ok(Some(ListNodesResult {
            nodes: vec![],
            current_proxy: None,
            status: res.status,
            error: Some("API 返回未授权（401/403），请到设置检查 Secret".into()),
            unauthorized: true,
            transport: Some("unix".into()),
        })),
        Ok(res) if (200..300).contains(&res.status) => {
            match parse_nodes_with_providers(&res.body, secret, unix_timeout, NodeTransport::Unix(sock_path.unwrap())).await {
                Ok(mut slim) => {
                    slim.status = res.status;
                    slim.transport = Some("unix".into());
                    Ok(Some(slim))
                }
                Err(e) => Ok(Some(ListNodesResult {
                    nodes: vec![],
                    current_proxy: None,
                    status: res.status,
                    error: Some(format!(
                        "解析 /proxies JSON 失败（Unix）: {e}；body {} 字节",
                        res.body.len()
                    )),
                    unauthorized: false,
                    transport: Some("unix".into()),
                })),
            }
        }
        Ok(_) | Err(_) => Ok(None),
    }
}

/// Fetch /proxies. When `sock_path` is set (Verge 2.5.6+ / Party), try Unix
/// first — TCP EC is often blank or stale. Otherwise TCP briefly, then Unix.
pub async fn list_nodes_async(
    host: &str,
    port: u16,
    secret: &str,
    timeout_ms: u64,
    sock_path: Option<&str>,
) -> Result<ListNodesResult, String> {
    let tcp_timeout = timeout_ms.min(2000).max(400);
    let unix_timeout = timeout_ms.max(tcp_timeout);
    let prefer_unix = sock_path.map(|s| !s.is_empty()).unwrap_or(false);

    if prefer_unix {
        if let Some(result) = try_list_nodes_unix(secret, sock_path, unix_timeout).await? {
            return Ok(result);
        }
        // Fall through to TCP for older Verge with live EC.
    }

    let tcp_attempt =
        http_via_tcp_async(host, port, "GET", "/proxies", None, secret, tcp_timeout).await;

    match tcp_attempt {
        Ok(res) if res.status == 401 || res.status == 403 => {
            return Ok(ListNodesResult {
                nodes: vec![],
                current_proxy: None,
                status: res.status,
                error: Some("API 返回未授权（401/403），请到设置检查 Secret".into()),
                unauthorized: true,
                transport: Some("tcp".into()),
            });
        }
        Ok(res) if (200..300).contains(&res.status) => {
            match parse_nodes_with_providers(&res.body, secret, tcp_timeout, NodeTransport::Tcp(host, port)).await {
                Ok(mut slim) => {
                    slim.status = res.status;
                    slim.transport = Some("tcp".into());
                    return Ok(slim);
                }
                Err(e) => {
                    return Ok(ListNodesResult {
                        nodes: vec![],
                        current_proxy: None,
                        status: res.status,
                        error: Some(format!(
                            "解析 /proxies JSON 失败（TCP）: {e}；body {} 字节",
                            res.body.len()
                        )),
                        unauthorized: false,
                        transport: Some("tcp".into()),
                    });
                }
            }
        }
        Ok(_) | Err(_) => {
            // Fall through to unix: connect failure or non-2xx (except 401/403 above).
        }
    }

    // TCP failed (or was skipped): unix fallback when sock configured.
    match try_list_nodes_unix(secret, sock_path, unix_timeout).await? {
        Some(result) => Ok(result),
        None => {
            let sock_hint = sock_path.filter(|s| !s.is_empty()).unwrap_or("(none)");
            if sock_path.map(|s| !s.is_empty()).unwrap_or(false) {
                Err(format!(
                    "TCP {host}:{port} 不可用，Unix {sock_hint} 也失败"
                ))
            } else {
                Err(format!(
                    "TCP {host}:{port} 不可用，且未配置 Unix 套接字（不会回退到 Verge 默认 sock）"
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A local controller fixture; no real proxy process or network settings are touched.
    struct ProviderController {
        port: u16,
        requests: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
        stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
        worker: Option<std::thread::JoinHandle<()>>,
    }

    impl ProviderController {
        fn start(providers_status: u16, providers_body: &str) -> Self {
            use std::io::{Read, Write};
            use std::sync::{atomic::{AtomicBool, Ordering}, Arc, Mutex};
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            listener.set_nonblocking(true).unwrap();
            let requests = Arc::new(Mutex::new(Vec::new()));
            let stop = Arc::new(AtomicBool::new(false));
            let (seen, done) = (requests.clone(), stop.clone());
            let providers_body = providers_body.to_string();
            let worker = std::thread::spawn(move || {
                while !done.load(Ordering::Relaxed) {
                    let Ok((mut stream, _)) = listener.accept() else {
                        std::thread::sleep(std::time::Duration::from_millis(5));
                        continue;
                    };
                    stream.set_read_timeout(Some(std::time::Duration::from_secs(2))).unwrap();
                    let mut request = Vec::new();
                    let mut buf = [0; 1024];
                    while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                        match stream.read(&mut buf) {
                            Ok(0) | Err(_) => break,
                            Ok(n) => request.extend_from_slice(&buf[..n]),
                        }
                    }
                    let line = String::from_utf8_lossy(&request).lines().next().unwrap_or("").to_string();
                    seen.lock().unwrap().push(line.clone());
                    let (status, body) = if line.starts_with("GET /proxies ") {
                        (200, r#"{"proxies":{"Proxy":{"type":"Selector","now":"provider-a","all":["static-a","provider-a"]},"static-a":{"type":"Shadowsocks"}}}"#)
                    } else if line.starts_with("GET /providers/proxies ") {
                        (providers_status, providers_body.as_str())
                    } else {
                        (405, r#"{"message":"fixture only permits discovery GETs"}"#)
                    };
                    let response = format!("HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                    let _ = stream.write_all(response.as_bytes());
                }
            });
            Self { port, requests, stop, worker: Some(worker) }
        }
    }

    impl Drop for ProviderController {
        fn drop(&mut self) {
            self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
            if let Some(worker) = self.worker.take() { worker.join().unwrap(); }
        }
    }

    #[test]
    fn list_nodes_includes_provider_leaves_and_resolves_current_without_switching() {
        let controller = ProviderController::start(200, r#"{"providers":{"subscription":{"vehicleType":"File","proxies":[{"name":"provider-a","type":"Vless","udp":true}]}}}"#);
        let result = tauri::async_runtime::block_on(list_nodes_async("127.0.0.1", controller.port, "", 1500, None)).unwrap();
        let names: Vec<_> = result.nodes.iter().map(|n| n.name.as_str()).collect();
        assert_eq!(names.len(), 2, "provider-only nodes must supplement existing static nodes");
        assert!(names.contains(&"provider-a") && names.contains(&"static-a"));
        assert_eq!(result.current_proxy.as_deref(), Some("provider-a"));
        assert!(result.error.is_none());
        assert_eq!(*controller.requests.lock().unwrap(), vec!["GET /proxies HTTP/1.1", "GET /providers/proxies HTTP/1.1"]);
    }

    #[test]
    fn provider_failure_keeps_static_nodes_and_reports_incomplete_list() {
        let controller = ProviderController::start(403, r#"{"message":"denied"}"#);
        let result = tauri::async_runtime::block_on(list_nodes_async("127.0.0.1", controller.port, "", 1500, None)).unwrap();
        assert_eq!(result.nodes.len(), 1);
        assert_eq!(result.nodes[0].name, "static-a");
        assert!(result.current_proxy.is_none());
        assert!(result.error.as_deref().unwrap().contains("HTTP 403"));
        assert!(!result.unauthorized, "a provider error must not discard a working root controller");
    }

    #[test]
    fn provider_merge_rejects_ambiguous_names_and_preserves_root_identity() {
        let mut root = serde_json::json!({"proxies": {
            "Proxy": {"type":"Selector","now":"duplicate","all":["duplicate","root","unique","invalid"]},
            "root": {"type":"Trojan"}
        }});
        let providers = serde_json::json!({"providers": {
            "virtual": {"vehicleType":"Compatible","proxies":[{"name":"unique","type":"Vless"}]},
            "one": {"vehicleType":"File","proxies":[{"name":"duplicate","type":"Vless"},{"name":"unique","type":"Vless","udp":true},{"name":"root","type":"Vless"},{"name":"invalid"},{"name":"unused","type":"Vless"}]},
            "two": {"vehicleType":"HTTP","proxies":[{"name":"duplicate","type":"Trojan"}]}
        }});
        merge_provider_nodes(&mut root, &providers).unwrap();
        let proxies = root["proxies"].as_object().unwrap();
        assert_eq!(proxies["root"]["type"], "Trojan");
        assert_eq!(proxies["unique"]["udp"], true);
        assert!(!proxies.contains_key("duplicate"));
        assert!(!proxies.contains_key("invalid"));
        assert!(!proxies.contains_key("unused"));
        assert!(resolve_current_proxy(proxies).is_none());
    }

    #[test]
    fn smoke_spawn_blocking_catch_unwind_maps_panic() {
        let join = tauri::async_runtime::block_on(async {
            tauri::async_runtime::spawn_blocking(|| {
                std::panic::catch_unwind(|| {
                    panic!("intentional smoke panic");
                })
                .map_err(|_| "caught panic".to_string())
                .and_then(|()| Ok::<(), String>(()))
            })
            .await
        });
        assert!(join.is_ok());
        let inner = join.unwrap();
        assert!(inner.is_err());
        assert!(inner.unwrap_err().contains("panic"));
    }

    #[test]
    fn slim_nodes_strips_groups_and_history_shape() {
        let body = r#"{
          "proxies": {
            "Proxy": {"type":"Selector","now":"hk-1","all":["hk-1","jp-1"]},
            "hk-1": {"type":"Shadowsocks","history":[{"time":"t","delay":12}]},
            "jp-1": {"type":"Vmess","history":[{"time":"t","delay":99}]},
            "DIRECT": {"type":"Direct"}
          }
        }"#;
        let slim = slim_nodes_from_proxies_json(body).unwrap();
        assert_eq!(slim.current_proxy.as_deref(), Some("hk-1"));
        let names: Vec<_> = slim.nodes.iter().map(|n| n.name.as_str()).collect();
        assert!(names.contains(&"hk-1"));
        assert!(names.contains(&"jp-1"));
        assert!(!names.iter().any(|n| *n == "Proxy" || *n == "DIRECT"));
    }

    #[test]
    fn slim_nodes_keeps_client_health() {
        // alive + last delay survive slimming: dead (delay 0), alive, and
        // never-tested (no history at all) must be distinguishable, and the
        // failure time must come through for freshness checks.
        let body = r#"{
          "proxies": {
            "Proxy": {"type":"Selector","now":"dead","all":["dead","ok","fresh"]},
            "dead": {"type":"Shadowsocks","alive":false,"history":[{"time":"t","delay":120},{"time":"t2","delay":0}]},
            "ok": {"type":"Vmess","alive":true,"history":[{"time":"t","delay":88}]},
            "fresh": {"type":"Trojan"}
          }
        }"#;
        let slim = slim_nodes_from_proxies_json(body).unwrap();
        let get = |n: &str| slim.nodes.iter().find(|x| x.name == n).unwrap();
        let dead = get("dead");
        assert_eq!(dead.alive, Some(false));
        assert_eq!(dead.last_delay, Some(0));
        assert_eq!(dead.last_delay_at.as_deref(), Some("t2"));
        let ok = get("ok");
        assert_eq!(ok.alive, Some(true));
        assert_eq!(ok.last_delay, Some(88));
        assert_eq!(ok.last_delay_at.as_deref(), Some("t"));
        let fresh = get("fresh");
        assert_eq!(fresh.alive, None);
        assert_eq!(fresh.last_delay, None);
        assert_eq!(fresh.last_delay_at, None);
    }

    #[test]
    fn slim_nodes_reads_all_six_capability_flags() {
        // A node with no flags (udp must NOT be assumed true), and a node with
        // the full set lit — proves each flag is read independently.
        let body = r#"{
          "proxies": {
            "Proxy": {"type":"Selector","now":"plain","all":["plain","rich"]},
            "plain": {"type":"Shadowsocks"},
            "rich": {"type":"Vless","udp":true,"xudp":true,"uot":true,"tfo":true,"smux":true,"mptcp":true}
          }
        }"#;
        let slim = slim_nodes_from_proxies_json(body).unwrap();
        let get = |n: &str| slim.nodes.iter().find(|x| x.name == n).unwrap();
        let p = get("plain");
        assert!(!p.udp && !p.xudp && !p.uot && !p.tfo && !p.smux && !p.mptcp);
        let r = get("rich");
        assert!(r.udp && r.xudp && r.uot && r.tfo && r.smux && r.mptcp);
    }

    #[test]
    fn slim_nodes_partial_flags_independent() {
        // Mirrors the real-world concern: some nodes have udp, some don't; a
        // node can have udp but lack xudp, etc. Flags must not cross-contaminate.
        let body = r#"{
          "proxies": {
            "Proxy": {"type":"Selector","now":"a","all":["a","b"]},
            "a": {"type":"Hysteria2","udp":true},
            "b": {"type":"Trojan","uot":true}
          }
        }"#;
        let slim = slim_nodes_from_proxies_json(body).unwrap();
        let a = slim.nodes.iter().find(|x| x.name == "a").unwrap();
        let b = slim.nodes.iter().find(|x| x.name == "b").unwrap();
        assert!(a.udp && !a.xudp && !a.uot);
        assert!(!b.udp && b.uot);
    }

    #[test]
    fn current_proxy_rule_mode_skips_direct_global_picks_节点选择() {
        // Clash Verge in rule mode: GLOBAL pseudo-group sits at DIRECT while the
        // user's real pick lives in the "节点选择" group. Must show HK17, not DIRECT.
        let body = r#"{
          "proxies": {
            "GLOBAL": {"type":"Selector","now":"DIRECT","all":["DIRECT","HK17"]},
            "节点选择": {"type":"Selector","now":"HK17","all":["HK17","SG2"]},
            "NETFLIX": {"type":"Selector","now":"SG2","all":["HK17","SG2"]},
            "HK17": {"type":"Hysteria2"},
            "SG2": {"type":"Vmess"},
            "DIRECT": {"type":"Direct"},
            "REJECT": {"type":"Reject"}
          }
        }"#;
        let slim = slim_nodes_from_proxies_json(body).unwrap();
        assert_eq!(slim.current_proxy.as_deref(), Some("HK17"));
    }

    #[test]
    fn current_proxy_global_mode_uses_global_real_leaf() {
        let body = r#"{
          "proxies": {
            "GLOBAL": {"type":"Selector","now":"jp-2","all":["jp-2","hk-1"]},
            "节点选择": {"type":"Selector","now":"hk-1","all":["jp-2","hk-1"]},
            "jp-2": {"type":"Shadowsocks"},
            "hk-1": {"type":"Vmess"},
            "DIRECT": {"type":"Direct"}
          }
        }"#;
        let slim = slim_nodes_from_proxies_json(body).unwrap();
        // Proxy absent, GLOBAL present with a real leaf → GLOBAL wins first.
        assert_eq!(slim.current_proxy.as_deref(), Some("jp-2"));
    }

    #[test]
    fn current_proxy_ambiguous_rule_groups_returns_none() {
        // No preferred group name matches and several sub-groups disagree →
        // honest None (UI shows "—") rather than guessing a random group.
        let body = r#"{
          "proxies": {
            "人工智能": {"type":"Selector","now":"us-1","all":["us-1"]},
            "NETFLIX": {"type":"Selector","now":"sg-9","all":["sg-9"]},
            "us-1": {"type":"Vmess"},
            "sg-9": {"type":"Trojan"},
            "DIRECT": {"type":"Direct"}
          }
        }"#;
        let slim = slim_nodes_from_proxies_json(body).unwrap();
        assert_eq!(slim.current_proxy, None);
    }

    #[test]
    fn current_proxy_pure_direct_mode_returns_none() {
        let body = r#"{
          "proxies": {
            "GLOBAL": {"type":"Selector","now":"DIRECT","all":["DIRECT"]},
            "DIRECT": {"type":"Direct"}
          }
        }"#;
        let slim = slim_nodes_from_proxies_json(body).unwrap();
        assert_eq!(slim.current_proxy, None);
    }

    #[test]
    fn current_proxy_custom_main_group_beats_policy_groups() {
        let body = r#"{
          "proxies": {
            "GLOBAL": {"type":"Selector","now":"DIRECT","all":["DIRECT","n0","n1","n2","n3","n4","n5"]},
            "机场": {"type":"Selector","now":"n3","all":["n0","n1","n2","n3","n4","n5"]},
            "NETFLIX": {"type":"Selector","now":"n1","all":["n1","n2"]},
            "AI": {"type":"Selector","now":"n2","all":["n2"]},
            "Telegram": {"type":"Selector","now":"n0","all":["n0","n4"]},
            "n0": {"type":"Vmess"},
            "n1": {"type":"Vmess"},
            "n2": {"type":"Vmess"},
            "n3": {"type":"Vmess"},
            "n4": {"type":"Vmess"},
            "n5": {"type":"Vmess"},
            "DIRECT": {"type":"Direct"}
          }
        }"#;
        let slim = slim_nodes_from_proxies_json(body).unwrap();
        assert_eq!(slim.current_proxy.as_deref(), Some("n3"));
    }

    #[test]
    fn current_proxy_emoji_node_select_beats_policy_groups() {
        let body = r#"{
          "proxies": {
            "GLOBAL": {"type":"Selector","now":"DIRECT","all":["DIRECT","HK-1"]},
            "🚀 节点选择": {"type":"Selector","now":"HK-1","all":["♻️ 自动选择","HK-1","JP-1","US-1","SG-1","TW-1","DIRECT"]},
            "♻️ 自动选择": {"type":"URLTest","now":"JP-1","all":["HK-1","JP-1","US-1","SG-1","TW-1"]},
            "NETFLIX": {"type":"Selector","now":"US-1","all":["US-1","SG-1"]},
            "🤖 AI": {"type":"Selector","now":"JP-1","all":["JP-1"]},
            "Telegram": {"type":"Selector","now":"SG-1","all":["SG-1","HK-1"]},
            "HK-1": {"type":"Hysteria2"},
            "JP-1": {"type":"Hysteria2"},
            "US-1": {"type":"Hysteria2"},
            "SG-1": {"type":"Hysteria2"},
            "TW-1": {"type":"Hysteria2"},
            "DIRECT": {"type":"Direct"}
          }
        }"#;
        let slim = slim_nodes_from_proxies_json(body).unwrap();
        assert_eq!(slim.current_proxy.as_deref(), Some("HK-1"));
    }

    #[test]
    fn current_proxy_nested_preferred_ignores_policy_groups() {
        let body = r#"{
          "proxies": {
            "Proxy": {"type":"Selector","now":"内层","all":["内层"]},
            "内层": {"type":"Selector","now":"再内层","all":["再内层"]},
            "再内层": {"type":"Selector","now":"hk-1","all":["hk-1"]},
            "NETFLIX": {"type":"Selector","now":"sg-9","all":["sg-9"]},
            "hk-1": {"type":"Vmess"},
            "sg-9": {"type":"Trojan"},
            "DIRECT": {"type":"Direct"}
          }
        }"#;
        let slim = slim_nodes_from_proxies_json(body).unwrap();
        assert_eq!(slim.current_proxy.as_deref(), Some("hk-1"));
    }

    #[test]
    fn current_proxy_main_group_follows_region_subgroup() {
        let body = r#"{
          "proxies": {
            "GLOBAL": {"type":"Selector","now":"DIRECT","all":["DIRECT"]},
            "🚀 节点选择": {"type":"Selector","now":"🇯🇵 日本节点","all":["🇭🇰 香港节点","🇯🇵 日本节点"]},
            "🇭🇰 香港节点": {"type":"Selector","now":"hk-1","all":["hk-1","hk-2","hk-3"]},
            "🇯🇵 日本节点": {"type":"Selector","now":"jp-9","all":["jp-9","jp-8"]},
            "NETFLIX": {"type":"Selector","now":"hk-1","all":["hk-1","jp-9"]},
            "hk-1": {"type":"Vmess"},
            "hk-2": {"type":"Vmess"},
            "hk-3": {"type":"Vmess"},
            "jp-9": {"type":"Vmess"},
            "jp-8": {"type":"Vmess"},
            "DIRECT": {"type":"Direct"}
          }
        }"#;
        let slim = slim_nodes_from_proxies_json(body).unwrap();
        assert_eq!(slim.current_proxy.as_deref(), Some("jp-9"));
    }

    #[test]
    fn current_proxy_main_group_on_direct_does_not_borrow_policy_node() {
        let body = r#"{
          "proxies": {
            "GLOBAL": {"type":"Selector","now":"DIRECT","all":["DIRECT"]},
            "🚀 节点选择": {"type":"Selector","now":"DIRECT","all":["DIRECT","HK-1","JP-1"]},
            "NETFLIX": {"type":"Selector","now":"HK-1","all":["HK-1"]},
            "AI": {"type":"Selector","now":"JP-1","all":["JP-1"]},
            "HK-1": {"type":"Vmess"},
            "JP-1": {"type":"Vmess"},
            "DIRECT": {"type":"Direct"}
          }
        }"#;
        let slim = slim_nodes_from_proxies_json(body).unwrap();
        assert_eq!(slim.current_proxy, None);
    }

    #[test]
    fn smoke_list_nodes_dead_tcp_missing_sock_no_panic() {
        let missing = "/tmp/clash-assist-no-such-mihomo.sock";
        let _ = std::fs::remove_file(missing);
        let result = std::panic::catch_unwind(|| {
            tauri::async_runtime::block_on(list_nodes_async(
                "127.0.0.1",
                1,
                "",
                800,
                Some(missing),
            ))
        });
        assert!(result.is_ok(), "list_nodes_async panicked");
        let inner = result.unwrap();
        assert!(
            inner.is_err(),
            "expected Err when TCP dead and sock missing, got {inner:?}"
        );
    }

    /// TCP dead + live Unix sock → leaf nodes > 0, transport unix, no app demo names.
    #[test]
    fn smoke_list_nodes_dead_tcp_live_sock_real_leaves() {
        use std::io::{Read, Write};
        use std::os::unix::net::UnixListener;
        use std::sync::mpsc;
        use std::thread;
        use std::time::Duration;

        let sock = format!(
            "/tmp/clash-assist-list-nodes-smoke-{}.sock",
            std::process::id()
        );
        let _ = std::fs::remove_file(&sock);

        let body = concat!(
            r#"{"proxies":{"Proxy":{"type":"Selector","now":"香港 HK-2-AT","all":["香港 HK-2-AT","日本 TY-4-HY2"]},"GLOBAL":{"type":"Selector","now":"香港 HK-2-AT","all":["香港 HK-2-AT","日本 TY-4-HY2"]},"香港 HK-2-AT":{"type":"Hysteria2","history":[{"time":"t","delay":40}]},"日本 TY-4-HY2":{"type":"Hysteria2","history":[{"time":"t","delay":55}]},"DIRECT":{"type":"Direct"}}}"#
        );
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );

        let listener = UnixListener::bind(&sock).expect("bind unix smoke sock");
        let (ready_tx, ready_rx) = mpsc::channel();
        let sock_path = sock.clone();
        let server = thread::spawn(move || {
            ready_tx.send(()).ok();
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf);
                let _ = stream.write_all(resp.as_bytes());
            }
            let _ = std::fs::remove_file(&sock_path);
        });
        ready_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("server ready");

        let result = std::panic::catch_unwind(|| {
            tauri::async_runtime::block_on(list_nodes_async(
                "127.0.0.1",
                1,
                "test-secret",
                2000,
                Some(&sock),
            ))
        });
        let _ = server.join();
        let _ = std::fs::remove_file(&sock);

        assert!(result.is_ok(), "list_nodes_async panicked");
        let inner = result.unwrap().expect("list_nodes should Ok via unix");
        assert_eq!(inner.transport.as_deref(), Some("unix"));
        assert!(
            inner.nodes.len() >= 2,
            "expected real leaves via unix, got {:?}",
            inner.nodes
        );
        let names: Vec<_> = inner.nodes.iter().map(|n| n.name.as_str()).collect();
        assert!(names.contains(&"香港 HK-2-AT"));
        assert!(names.contains(&"日本 TY-4-HY2"));
        assert!(
            !names.iter().any(|n| n.contains("香港 01 | Hysteria2")
                || n.contains("东京 Premium")
                || n.contains("Singapore IEPL")),
            "demo mock names must not appear: {names:?}"
        );
        
        assert!(!inner.unauthorized);
    }

    /// TCP dead + live Unix sock returning **chunked** body with Chinese names.
    #[test]
    fn smoke_list_nodes_unix_chunked_chinese_names() {
        use std::io::{Read, Write};
        use std::os::unix::net::UnixListener;
        use std::sync::mpsc;
        use std::thread;
        use std::time::Duration;

        let sock = format!(
            "/tmp/clash-assist-list-nodes-chunked-{}.sock",
            std::process::id()
        );
        let _ = std::fs::remove_file(&sock);

        let json = concat!(
            r#"{"proxies":{"Proxy":{"type":"Selector","now":"香港 HK-2-AT","all":["香港 HK-2-AT","日本 TY-4-HY2"]},"香港 HK-2-AT":{"type":"Hysteria2"},"日本 TY-4-HY2":{"type":"Hysteria2"},"DIRECT":{"type":"Direct"}}}"#
        );
        let json_b = json.as_bytes();
        // Split inside multi-byte UTF-8 so char-based decode would corrupt.
        let split_at = (0..json_b.len())
            .find(|&i| !json.is_char_boundary(i))
            .expect("fixture must contain multi-byte UTF-8");
        let (a, b) = json_b.split_at(split_at);
        let mut chunked_body = Vec::new();
        chunked_body.extend_from_slice(format!("{:x}\r\n", a.len()).as_bytes());
        chunked_body.extend_from_slice(a);
        chunked_body.extend_from_slice(b"\r\n");
        chunked_body.extend_from_slice(format!("{:x}\r\n", b.len()).as_bytes());
        chunked_body.extend_from_slice(b);
        chunked_body.extend_from_slice(b"\r\n0\r\n\r\n");

        let mut resp = Vec::new();
        resp.extend_from_slice(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n",
        );
        resp.extend_from_slice(&chunked_body);

        let listener = UnixListener::bind(&sock).expect("bind");
        let (ready_tx, ready_rx) = mpsc::channel();
        let sock_path = sock.clone();
        let server = thread::spawn(move || {
            ready_tx.send(()).ok();
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 8192];
                let n = stream.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                assert!(
                    req.to_ascii_lowercase().contains("accept-encoding: identity"),
                    "unix request must ask for identity encoding, got:\n{req}"
                );
                let _ = stream.write_all(&resp);
            }
            let _ = std::fs::remove_file(&sock_path);
        });
        ready_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("server ready");

        let result = std::panic::catch_unwind(|| {
            tauri::async_runtime::block_on(list_nodes_async(
                "127.0.0.1",
                1,
                "test-secret",
                2000,
                Some(&sock),
            ))
        });
        let _ = server.join();
        let _ = std::fs::remove_file(&sock);

        assert!(result.is_ok(), "list_nodes_async panicked");
        let inner = result.unwrap().expect("list_nodes should Ok via unix chunked");
        assert_eq!(inner.transport.as_deref(), Some("unix"));
        assert!(
            inner.error.is_none(),
            "expected successful parse, got error {:?}",
            inner.error
        );
        let names: Vec<_> = inner.nodes.iter().map(|n| n.name.as_str()).collect();
        assert!(names.contains(&"香港 HK-2-AT"), "{names:?}");
        assert!(names.contains(&"日本 TY-4-HY2"), "{names:?}");
    }

    #[test]
    fn slim_parse_fail_returns_ok_with_error_via_list_nodes_unix() {
        use std::io::{Read, Write};
        use std::os::unix::net::UnixListener;
        use std::sync::mpsc;
        use std::thread;
        use std::time::Duration;

        let sock = format!(
            "/tmp/clash-assist-list-nodes-badjson-{}.sock",
            std::process::id()
        );
        let _ = std::fs::remove_file(&sock);

        // Valid HTTP but invalid JSON body — should Ok with error, not Err.
        let body = "{not-json";
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );

        let listener = UnixListener::bind(&sock).expect("bind");
        let (ready_tx, ready_rx) = mpsc::channel();
        let sock_path = sock.clone();
        let server = thread::spawn(move || {
            ready_tx.send(()).ok();
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf);
                let _ = stream.write_all(resp.as_bytes());
            }
            let _ = std::fs::remove_file(&sock_path);
        });
        ready_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("server ready");

        let result = tauri::async_runtime::block_on(list_nodes_async(
            "127.0.0.1",
            1,
            "",
            2000,
            Some(&sock),
        ));
        let _ = server.join();
        let _ = std::fs::remove_file(&sock);

        let inner = result.expect("should Ok with parse error detail, not Err");
        assert_eq!(inner.status, 200);
        assert!(inner.nodes.is_empty());
        let err = inner.error.expect("error detail");
        assert!(
            err.contains("解析 /proxies JSON 失败"),
            "unexpected error: {err}"
        );
    }
}
