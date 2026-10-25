//! `/proxies` handling: slim the controller's proxy JSON down to real leaf
//! nodes (dropping junk names and history arrays, keeping only the client's
//! last delay result) plus a table of the policy groups, and the TCP→Unix
//! fallback that fetches it for the node list.
//!
//! "Which node is currently in use" is decided by the frontend only
//! (`src/lib/mihomo/currentProxy.ts`). Rust ships `now` + `all` for every
//! group and does not resolve anything itself — the one implementation of that
//! rule used to exist twice, and the two copies had already drifted.

use serde_json::Value;

use super::transport::{http_via_tcp_async, http_via_unix};
use super::types::{ListNodesResult, SlimGroup, SlimNode, IGNORE_PROXY_TYPES, JUNK_NAME_KEYWORDS};

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

/// Mirror of TS `isGroupType`: the canonical spellings plus a case-insensitive
/// fallback. mihomo emits CamelCase group types, but a third-party core that
/// emitted `selector` would otherwise be dropped from the summary here while
/// TS still treated it as a group — i.e. a fresh drift source.
fn is_group_type(t: &str) -> bool {
    GROUP_TYPES.contains(&t)
        || matches!(
            t.to_ascii_lowercase().as_str(),
            "selector" | "urltest" | "fallback" | "loadbalance"
        )
}

/// Ship every policy group (`name` / `type` / `now` / full `all`) to the frontend.
///
/// This is the whole of Rust's remaining job for "which node is in use":
/// **transport, not judgement**. The decision lives only in TS
/// `src/lib/mihomo/currentProxy.ts`. `all` is deliberately not reduced to a
/// leaf count — counting real leaves needs the leaf predicate, and holding that
/// predicate in one place is exactly what prevents the two ends from picking
/// different nodes (see the `Select` / `select` drift pinned by
/// `tests/fixtures/proxies/manifest.json`).
fn group_summaries(proxies: &serde_json::Map<String, Value>) -> Vec<SlimGroup> {
    let mut groups = Vec::new();
    for (name, p) in proxies {
        let group_type = p.get("type").and_then(Value::as_str).unwrap_or("");
        if !is_group_type(group_type) {
            continue;
        }
        let all = p
            .get("all")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        groups.push(SlimGroup {
            name: name.clone(),
            group_type: group_type.to_string(),
            now: p.get("now").and_then(Value::as_str).map(str::to_string),
            all,
        });
    }
    groups
}

/// Borrow the `proxies` object out of a parsed `/proxies` body.
fn proxies_of(root: &Value) -> Result<&serde_json::Map<String, Value>, String> {
    root.get("proxies")
        .and_then(Value::as_object)
        .ok_or_else(|| "missing proxies object".to_string())
}

/// Build the frontend payload from an **already parsed** `proxies` map.
fn slim_list_from_proxies(proxies: &serde_json::Map<String, Value>) -> ListNodesResult {
    let groups = group_summaries(proxies);

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

    ListNodesResult {
        nodes,
        groups,
        status: 200,
        error: None,
        unauthorized: false,
        transport: None,
    }
}

/// Parse a raw `/proxies` body into the frontend payload (slim nodes — no
/// history arrays, only the last delay result + alive flag — plus the group
/// table).
///
/// Test-only on purpose: the production node-list path already has the body
/// parsed for the provider merge and feeds [`slim_list_from_proxies`] directly.
/// Going through a string there used to mean re-serializing the whole tree and
/// parsing it back — see `parse_nodes_with_providers`.
#[cfg(test)]
pub fn slim_nodes_from_proxies_json(body: &str) -> Result<ListNodesResult, String> {
    let root: Value = serde_json::from_str(body).map_err(|e| format!("json: {e}"))?;
    Ok(slim_list_from_proxies(proxies_of(&root)?))
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

/// Insert the uniquely-resolvable provider entries for `missing` into `root`.
///
/// `missing` is passed in rather than recomputed here, and the return value is
/// how many names actually landed — the caller needs a residual count and can
/// get it by subtraction instead of re-scanning the whole proxy tree.
fn merge_provider_nodes(
    root: &mut Value,
    provider_root: &Value,
    missing: &std::collections::BTreeSet<String>,
) -> Result<usize, String> {
    let providers = provider_root.get("providers").and_then(Value::as_object)
        .ok_or_else(|| "missing providers object".to_string())?;
    let mut inserted = 0usize;
    for name in missing {
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
                .insert(name.to_string(), candidates[0].clone());
            inserted += 1;
        }
    }
    Ok(inserted)
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
    let missing_before = missing_provider_names(&root)?;
    let mut inserted = 0usize;
    if !missing_before.is_empty() {
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
                    .and_then(|providers| merge_provider_nodes(&mut root, &providers, &missing_before)) {
                    Ok(n) => inserted = n,
                    Err(_) => provider_error = Some("订阅节点响应无法解析".to_string()),
                }
            }
            Ok(response) => provider_error = Some(format!("订阅节点接口返回 HTTP {}", response.status)),
            Err(_) => provider_error = Some("暂时读不到订阅节点接口".to_string()),
        }
    }
    let mut result = slim_list_from_proxies(proxies_of(&root)?);
    // merge 只会补进 missing_before 里的名字，所以剩余数按插入数直接减，
    // 不必再扫一遍整棵代理树（大订阅下这是第三遍全量扫描）。
    let missing = missing_before.len() - inserted;
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
            groups: vec![],
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
                    groups: vec![],
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
                groups: vec![],
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
                        groups: vec![],
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
    fn list_nodes_includes_provider_leaves_and_ships_the_group_table() {
        let controller = ProviderController::start(200, r#"{"providers":{"subscription":{"vehicleType":"File","proxies":[{"name":"provider-a","type":"Vless","udp":true}]}}}"#);
        let result = tauri::async_runtime::block_on(list_nodes_async("127.0.0.1", controller.port, "", 1500, None)).unwrap();
        let names: Vec<_> = result.nodes.iter().map(|n| n.name.as_str()).collect();
        assert_eq!(names.len(), 2, "provider-only nodes must supplement existing static nodes");
        assert!(names.contains(&"provider-a") && names.contains(&"static-a"));
        // The merged provider node must be reachable through the group table too,
        // otherwise the frontend cannot follow `now` to a real leaf.
        assert_eq!(result.groups.len(), 1);
        assert_eq!(result.groups[0].name, "Proxy");
        assert_eq!(result.groups[0].now.as_deref(), Some("provider-a"));
        assert_eq!(result.groups[0].all, vec!["static-a".to_string(), "provider-a".to_string()]);
        assert!(result.error.is_none());
        assert_eq!(*controller.requests.lock().unwrap(), vec!["GET /proxies HTTP/1.1", "GET /providers/proxies HTTP/1.1"]);
    }

    #[test]
    fn provider_failure_keeps_static_nodes_and_reports_incomplete_list() {
        let controller = ProviderController::start(403, r#"{"message":"denied"}"#);
        let result = tauri::async_runtime::block_on(list_nodes_async("127.0.0.1", controller.port, "", 1500, None)).unwrap();
        assert_eq!(result.nodes.len(), 1);
        assert_eq!(result.nodes[0].name, "static-a");
        assert_eq!(result.groups.len(), 1, "the group table must still ship when providers fail");
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
        let missing = missing_provider_names(&root).unwrap();
        let inserted = merge_provider_nodes(&mut root, &providers, &missing).unwrap();
        // 「duplicate」有两个候选、「invalid」没有 type、「unused」没被任何组引用 → 只补进 unique。
        assert_eq!(inserted, 1, "只有能唯一识别的名字才该补进来");
        assert_eq!(missing.len(), 3, "duplicate / unique / invalid 都缺失，unused 不算");
        let proxies = root["proxies"].as_object().unwrap();
        assert_eq!(proxies["root"]["type"], "Trojan");
        assert_eq!(proxies["unique"]["udp"], true);
        assert!(!proxies.contains_key("duplicate"));
        assert!(!proxies.contains_key("invalid"));
        assert!(!proxies.contains_key("unused"));
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
    fn slim_nodes_excludes_groups_from_nodes_but_ships_their_table() {
        let body = r#"{
          "proxies": {
            "Proxy": {"type":"Selector","now":"hk-1","all":["hk-1","jp-1"]},
            "hk-1": {"type":"Shadowsocks","history":[{"time":"t","delay":12}]},
            "jp-1": {"type":"Vmess","history":[{"time":"t","delay":99}]},
            "DIRECT": {"type":"Direct"}
          }
        }"#;
        let slim = slim_nodes_from_proxies_json(body).unwrap();
        let names: Vec<_> = slim.nodes.iter().map(|n| n.name.as_str()).collect();
        assert!(names.contains(&"hk-1"));
        assert!(names.contains(&"jp-1"));
        assert!(!names.iter().any(|n| *n == "Proxy" || *n == "DIRECT"));
        // The group itself is not a node, but the frontend still needs it
        // verbatim — this table is the only input to "which node is in use".
        assert_eq!(slim.groups.len(), 1);
        assert_eq!(slim.groups[0].name, "Proxy");
        assert_eq!(slim.groups[0].group_type, "Selector");
        assert_eq!(slim.groups[0].now.as_deref(), Some("hk-1"));
        assert_eq!(
            slim.groups[0].all,
            vec!["hk-1".to_string(), "jp-1".to_string()]
        );
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

    /// 双端共享语料（`tests/fixtures/proxies/cases.json`）在 Rust 侧的契约：
    /// **组表搬运无损**。
    ///
    /// 同一份文件也喂给 TS（`src/lib/mihomo/currentProxyFixtures.test.ts`），那边
    /// 断言 `expectedCurrent`——当前节点判定的唯一 spec 在 TS 侧。Rust 不再参与判定，
    /// 只核对交给前端的组表：`type` / `now` / 完整的 `all` 少一条，前端就跟不到真叶，
    /// 「使用中」徽标落错卡、深测后也切不回原节点。
    ///
    /// 期望值按 `GROUP_TYPES` 的字面拼写从 `proxies` 里挑组（不借用被测的
    /// `is_group_type`），否则这条测试就只是在跟自己对暗号。
    ///
    /// 历史背景：这套判定曾在 Rust / TS 各存一份且已漂移到挑出不同节点（组名恰好
    /// `Select` / `select` 时 Rust 曾漏掉 rank 3），0.2.7 收敛为单份。
    #[test]
    fn fixture_group_table_is_transported_losslessly() {
        let corpus: Value =
            serde_json::from_str(include_str!("../../../tests/fixtures/proxies/cases.json"))
                .expect("cases.json 必须是合法 JSON");
        let cases = corpus
            .get("cases")
            .and_then(Value::as_array)
            .expect("cases 必须是数组");
        assert!(
            cases.len() >= 15,
            "共享语料不得少于 15 条（当前 {} 条），删用例等于让对拍空转",
            cases.len()
        );

        type Table = std::collections::BTreeMap<String, (String, Option<String>, Vec<String>)>;
        let mut failures = Vec::new();
        for c in cases {
            let name = c.get("name").and_then(Value::as_str).unwrap_or("?");
            let note = c.get("note").and_then(Value::as_str).unwrap_or("");
            let proxies = c
                .get("proxies")
                .and_then(Value::as_object)
                .unwrap_or_else(|| panic!("用例 {name} 缺少 proxies 对象"));
            let body = serde_json::to_string(&serde_json::json!({ "proxies": proxies })).unwrap();
            let slim = slim_nodes_from_proxies_json(&body)
                .unwrap_or_else(|e| panic!("用例 {name} 解析失败: {e}"));

            let got: Table = slim
                .groups
                .iter()
                .map(|g| {
                    (
                        g.name.clone(),
                        (g.group_type.clone(), g.now.clone(), g.all.clone()),
                    )
                })
                .collect();
            let want: Table = proxies
                .iter()
                .filter(|(_, p)| GROUP_TYPES.contains(&p.get("type").and_then(Value::as_str).unwrap_or("")))
                .map(|(gname, p)| {
                    let all = p
                        .get("all")
                        .and_then(Value::as_array)
                        .map(|arr| {
                            arr.iter().filter_map(Value::as_str).map(str::to_string).collect()
                        })
                        .unwrap_or_default();
                    (
                        gname.clone(),
                        (
                            p.get("type").and_then(Value::as_str).unwrap_or("").to_string(),
                            p.get("now").and_then(Value::as_str).map(str::to_string),
                            all,
                        ),
                    )
                })
                .collect();
            assert!(
                !got.is_empty() || want.is_empty(),
                "用例 {name}: 语料里有策略组，但 Rust 一条都没搬出去"
            );
            // 策略组绝不能同时出现在节点表里：它是判定的输入，不是候选节点。
            for gname in want.keys() {
                if slim.nodes.iter().any(|n| &n.name == gname) {
                    failures.push(format!("{name}（{note}）: 组 {gname} 混进了 nodes"));
                }
            }
            if got != want {
                failures.push(format!(
                    "{name}（{note}）: 组表不一致\n  Rust = {got:#?}\n  期望 = {want:#?}"
                ));
            }
        }
        assert!(
            failures.is_empty(),
            "{} 条用例的组表搬运有误：\n{}",
            failures.len(),
            failures.join("\n")
        );
    }

    /// 前端只认 `groups` 这一份组表；Rust 侧不得再泄漏任何「已判定」的当前节点字段
    /// ——那等于把刚退役的第二套判定又请回来。
    #[test]
    fn list_nodes_payload_exposes_groups_not_a_resolved_current_proxy() {
        let body = r#"{
          "proxies": {
            "Proxy": {"type":"Selector","now":"hk-1","all":["hk-1"]},
            "hk-1": {"type":"Shadowsocks"}
          }
        }"#;
        let slim = slim_nodes_from_proxies_json(body).unwrap();
        let json = serde_json::to_string(&slim).unwrap();
        assert!(json.contains("\"groups\""), "组表必须序列化出去：{json}");
        assert!(!json.contains("currentProxy"), "Rust 不再判定当前节点：{json}");
        // 键名必须就是前端 `SlimGroupLike` 的字段名。错一个 TS 侧静默读到
        // undefined，判定退化成「认不出主组 → 使用中显示未知」：测试不炸，只有 UI 看得见。
        assert!(json.contains("\"type\":\"Selector\""), "groupType 要序列化成 type：{json}");
        assert!(json.contains("\"name\":\"Proxy\""), "组名键要是 name：{json}");
        assert!(json.contains("\"now\":\"hk-1\""), "当前指向键要是 now：{json}");
        assert!(json.contains("\"all\":[\"hk-1\"]"), "成员表键要是 all：{json}");
        assert!(!json.contains("groupType"), "不得泄漏 Rust 侧内部字段名：{json}");
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
