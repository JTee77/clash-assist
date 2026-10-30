//! macOS DNS resolver listing via `scutil --dns`.
//! Prefer scoped tunnel/VPN resolvers when present; otherwise unique nameservers.

use serde::Serialize;
use std::collections::BTreeSet;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DnsResolversResult {
    pub resolvers: Vec<String>,
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_hint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Result of a `dig TXT whoami.ds.akahelp.net` round-trip: the ground-truth of
/// where DNS traffic actually egresses and which recursive resolver served it.
/// Unlike `scutil --dns` (which only reads *configured* resolvers), this proves
/// the *effective* path — the whole point of the deterministic DNS-leak check.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DnsWhoamiResult {
    pub ok: bool,
    /// Public IP the authoritative server saw (the `"ip"` field).
    pub client_ip: Option<String>,
    /// Recursive nameserver that forwarded the query (the `"ns"` field).
    pub resolver_ns: Option<String>,
    /// EDNS Client Subnet prefix if returned (the `"ecs"` field).
    pub ecs: Option<String>,
    /// Which upstream we forced the query through: "system" or an @resolver IP.
    pub via: String,
    pub raw: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

fn unquote(s: &str) -> String {
    s.trim().trim_matches('"').trim_matches('\'').to_string()
}

/// Parse `dig +short TXT whoami.ds.akahelp.net` output into
/// (client_ip, resolver_ns, ecs). Field lines look like `"ip" "1.2.3.4"`.
/// A bare quoted IP line with no key is treated as the client ip fallback.
pub fn parse_whoami_txt(raw: &str) -> (Option<String>, Option<String>, Option<String>) {
    let mut ip = None;
    let mut ns = None;
    let mut ecs = None;
    for line in raw.lines() {
        let tokens: Vec<String> = line.split_whitespace().map(unquote).collect();
        match tokens.as_slice() {
            [k, v] => match k.to_ascii_lowercase().as_str() {
                "ip" => ip = Some(v.clone()),
                "ns" => ns = Some(v.clone()),
                "ecs" => ecs = Some(v.clone()),
                _ => {}
            },
            // Bare value (some whoami services echo just the client IP).
            [only] if ip.is_none() && looks_like_ip(only) => {
                ip = Some(only.clone());
            }
            _ => {}
        }
    }
    (ip, ns, ecs)
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))] // scutil 解析只在 macOS 用；单测全平台跑
#[derive(Debug, Default, Clone)]
struct ResolverBlock {
    nameservers: Vec<String>,
    if_index_line: Option<String>,
    scoped: bool,
}

fn looks_like_ip(s: &str) -> bool {
    let t = s.trim();
    if t.is_empty() {
        return false;
    }
    if t.parse::<std::net::Ipv4Addr>().is_ok() {
        return true;
    }
    if t.parse::<std::net::Ipv6Addr>().is_ok() {
        return true;
    }
    false
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))] // scutil 解析只在 macOS 用；单测全平台跑
fn is_tunnel_iface(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    // scutil: "if_index : 15 (utun4)" etc.
    ["utun", "ipsec", "ppp", "wg", "tun", "tap", "wireguard"]
        .iter()
        .any(|k| lower.contains(k))
}

/// Parse `scutil --dns` text into preferred resolver IP list.
///
/// Strategy:
/// 1. Split into resolver blocks (lines starting with `resolver #`).
/// 2. Collect nameserver[*] IPs per block; note scoped + if_index.
/// 3. If any block looks like tunnel/VPN (utun/ipsec/ppp/…), prefer those nameservers.
/// 4. Else if scoped resolvers exist, prefer those.
/// 5. Else return all unique nameservers (BTreeSet → stable sorted Vec).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))] // scutil 解析只在 macOS 用；单测全平台跑
pub fn parse_scutil_dns(raw: &str) -> (Vec<String>, String) {
    let mut blocks: Vec<ResolverBlock> = Vec::new();
    let mut current: Option<ResolverBlock> = None;

    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("resolver #") {
            if let Some(b) = current.take() {
                blocks.push(b);
            }
            current = Some(ResolverBlock::default());
            continue;
        }
        let Some(block) = current.as_mut() else {
            continue;
        };
        if let Some(rest) = trimmed.strip_prefix("nameserver[") {
            // nameserver[0] : 1.1.1.1
            if let Some(colon) = rest.find(':') {
                let ip = rest[colon + 1..].trim();
                if looks_like_ip(ip) {
                    block.nameservers.push(ip.to_string());
                }
            }
        } else if trimmed.starts_with("if_index") {
            block.if_index_line = Some(trimmed.to_string());
        } else if trimmed.starts_with("flags") && trimmed.to_ascii_lowercase().contains("scoped") {
            block.scoped = true;
        }
    }
    if let Some(b) = current.take() {
        blocks.push(b);
    }

    let tunnel: Vec<&ResolverBlock> = blocks
        .iter()
        .filter(|b| {
            !b.nameservers.is_empty()
                && b.if_index_line
                    .as_deref()
                    .map(is_tunnel_iface)
                    .unwrap_or(false)
        })
        .collect();

    let preferred: Vec<&ResolverBlock> = if !tunnel.is_empty() {
        tunnel
    } else {
        let scoped: Vec<&ResolverBlock> = blocks
            .iter()
            .filter(|b| b.scoped && !b.nameservers.is_empty())
            .collect();
        if !scoped.is_empty() {
            scoped
        } else {
            blocks.iter().filter(|b| !b.nameservers.is_empty()).collect()
        }
    };

    let mut set = BTreeSet::new();
    for b in &preferred {
        for ns in &b.nameservers {
            set.insert(ns.clone());
        }
    }
    let resolvers: Vec<String> = set.into_iter().collect();

    let source = if preferred.iter().any(|b| {
        b.if_index_line
            .as_deref()
            .map(is_tunnel_iface)
            .unwrap_or(false)
    }) {
        "scutil-scoped-tunnel".to_string()
    } else if preferred.iter().any(|b| b.scoped) {
        "scutil-scoped".to_string()
    } else {
        "scutil-all".to_string()
    };

    (resolvers, source)
}

/// Char-boundary-safe hint truncation: never slices inside a multi-byte
/// UTF-8 char (a raw `&raw[..400]` could panic on multibyte output).
fn truncate_hint(raw: &str, max_chars: usize) -> String {
    if raw.chars().count() <= max_chars {
        return raw.to_string();
    }
    let head: String = raw.chars().take(max_chars).collect();
    format!("{head}… ({} bytes)", raw.len())
}

pub fn list_dns_resolvers_blocking() -> DnsResolversResult {
    #[cfg(not(target_os = "macos"))]
    {
        DnsResolversResult {
            resolvers: vec![],
            source: "unsupported-platform".into(),
            raw_hint: None,
            error: Some(
                "DNS 解析器列表依赖 macOS `scutil --dns`；当前平台不可用。".into(),
            ),
        }
    }

    #[cfg(target_os = "macos")]
    {
        use std::process::Command;

        let output = match Command::new("scutil").arg("--dns").output() {
            Ok(o) => o,
            Err(e) => {
                return DnsResolversResult {
                    resolvers: vec![],
                    source: "scutil".into(),
                    raw_hint: None,
                    error: Some(format!("执行 scutil --dns 失败: {e}")),
                };
            }
        };

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return DnsResolversResult {
                resolvers: vec![],
                source: "scutil".into(),
                raw_hint: None,
                error: Some(format!(
                    "scutil --dns 退出码 {:?}: {}",
                    output.status.code(),
                    stderr.trim()
                )),
            };
        }

        let raw = String::from_utf8_lossy(&output.stdout).into_owned();
        let (resolvers, source) = parse_scutil_dns(&raw);
        let hint = truncate_hint(&raw, 400);

        if resolvers.is_empty() {
            return DnsResolversResult {
                resolvers,
                source,
                raw_hint: Some(hint),
                error: Some("scutil --dns 已执行但未解析到 nameserver".into()),
            };
        }

        DnsResolversResult {
            resolvers,
            source,
            raw_hint: Some(hint),
            error: None,
        }
    }
}

/// Discover the system's default DNS resolver.
/// - macOS: first nameserver from `scutil --dns` (reuses `list_dns_resolvers_blocking`).
/// - Linux: first `nameserver` from `/etc/resolv.conf`.
/// - Windows: fallback to well-known public resolver (8.8.8.8) — the OS DNS API
///   requires the `windows` crate or `GetAdaptersInfo` and is a separate concern.
fn find_system_resolver() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        let r = list_dns_resolvers_blocking();
        r.resolvers.into_iter().next()
    }
    #[cfg(target_os = "linux")]
    {
        #[allow(clippy::disallowed_methods, reason = "仅 Linux/Unix，不在 S-R1 的 Windows 风险面")]
        if let Ok(content) = std::fs::read_to_string("/etc/resolv.conf") {
            for line in content.lines() {
                if let Some(rest) = line.strip_prefix("nameserver") {
                    let ip = rest.trim();
                    if !ip.is_empty() {
                        return Some(ip.to_string());
                    }
                }
            }
        }
        None
    }
    #[cfg(target_os = "windows")]
    {
        // Best-effort fallback; full Windows adapter enumeration deferred.
        Some("8.8.8.8".to_string())
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        None
    }
}

/// Blocking raw-UDP DNS TXT query to `whoami.ds.akahelp.net`.
/// `resolver` = Some("1.1.1.1") to force a specific recursive resolver;
/// None uses the system default path (discovered via `find_system_resolver`).
pub fn dns_whoami_blocking(
    resolver: Option<&str>,
    timeout_ms: u64,
    cancelled: &dyn Fn() -> bool,
) -> DnsWhoamiResult {
    use crate::dns_query;
    use std::time::Duration;

    fn whoami_err(via: String, msg: String) -> DnsWhoamiResult {
        DnsWhoamiResult {
            ok: false,
            client_ip: None,
            resolver_ns: None,
            ecs: None,
            via,
            raw: String::new(),
            error: Some(msg),
        }
    }

    if cancelled() {
        return whoami_err("system".to_string(), "已取消".to_string());
    }

    let host = "whoami.ds.akahelp.net";
    let timeout = Duration::from_millis(timeout_ms.max(500));

    let server = match resolver {
        Some(r) => r.to_string(),
        None => match find_system_resolver() {
            Some(s) => s,
            None => return whoami_err("system".to_string(), "无法确定系统 DNS 解析器地址".to_string()),
        },
    };

    let via = server.clone();

    match dns_query::dns_txt_lookup(host, &server, timeout, cancelled) {
        Ok(txt_strings) => {
            let raw = txt_strings.join("\n");
            let (client_ip, resolver_ns, ecs) = parse_whoami_txt(&raw);
            let ok = client_ip.is_some() || resolver_ns.is_some();
            DnsWhoamiResult {
                ok,
                client_ip,
                resolver_ns,
                ecs,
                via,
                raw: truncate_hint(&raw, 300),
                error: if ok {
                    None
                } else {
                    Some("DNS 查询已返回但未解析到 ip/ns 字段".into())
                },
            }
        }
        Err(dns_query::DnsError::Cancelled) => whoami_err(via, "已取消".to_string()),
        Err(e) => whoami_err(via, format!("UDP DNS 查询失败: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_whoami_full_fields() {
        let raw = "\"ns\" \"172.68.41.102\"\n\"ip\" \"146.19.163.158\"\n\"ecs\" \"146.19.163.0/24/24\"\n";
        let (ip, ns, ecs) = parse_whoami_txt(raw);
        assert_eq!(ip.as_deref(), Some("146.19.163.158"));
        assert_eq!(ns.as_deref(), Some("172.68.41.102"));
        assert_eq!(ecs.as_deref(), Some("146.19.163.0/24/24"));
    }

    #[test]
    fn parse_whoami_bare_ip_fallback() {
        let (ip, ns, ecs) = parse_whoami_txt("\"203.0.113.7\"\n");
        assert_eq!(ip.as_deref(), Some("203.0.113.7"));
        assert_eq!(ns, None);
        assert_eq!(ecs, None);
    }

    #[test]
    fn parse_whoami_ignores_unknown_and_empty() {
        let (ip, ns, ecs) = parse_whoami_txt("\"id\" \"1234\"\n\n");
        assert_eq!(ip, None);
        assert_eq!(ns, None);
        assert_eq!(ecs, None);
    }

    const FIXTURE: &str = r#"DNS configuration

resolver #1
  nameserver[0] : 192.168.1.1
  nameserver[1] : 8.8.8.8
  flags    : Request A records, Request AAAA records
  reach    : 0x00000002 (Reachable)

resolver #2
  domain   : local
  options  : mdns
  timeout  : 5
  flags    : Request A records
  reach    : 0x00000000 (Not Reachable)
  order    : 300000

DNS configuration (for scoped queries)

resolver #1
  nameserver[0] : 10.64.0.1
  if_index : 15 (utun4)
  flags    : Scoped, Request A records, Request AAAA records
  reach    : 0x00000002 (Reachable)

resolver #2
  nameserver[0] : 192.168.1.1
  if_index : 6 (en0)
  flags    : Scoped, Request A records
  reach    : 0x00000002 (Reachable)
"#;

    #[test]
    fn parse_prefers_scoped_tunnel_nameservers() {
        let (resolvers, source) = parse_scutil_dns(FIXTURE);
        assert_eq!(resolvers, vec!["10.64.0.1".to_string()]);
        assert_eq!(source, "scutil-scoped-tunnel");
    }

    #[test]
    fn parse_all_unique_when_no_tunnel() {
        let raw = r#"DNS configuration

resolver #1
  nameserver[0] : 1.1.1.1
  nameserver[1] : 8.8.8.8
  flags    : Request A records

resolver #2
  nameserver[0] : 1.1.1.1
  domain   : lan
"#;
        let (resolvers, source) = parse_scutil_dns(raw);
        assert_eq!(
            resolvers,
            vec!["1.1.1.1".to_string(), "8.8.8.8".to_string()]
        );
        assert_eq!(source, "scutil-all");
    }

    #[test]
    fn parse_scoped_without_tunnel() {
        let raw = r#"
DNS configuration (for scoped queries)

resolver #1
  nameserver[0] : 192.168.0.1
  if_index : 6 (en0)
  flags    : Scoped, Request A records
"#;
        let (resolvers, source) = parse_scutil_dns(raw);
        assert_eq!(resolvers, vec!["192.168.0.1".to_string()]);
        assert_eq!(source, "scutil-scoped");
    }

    #[test]
    fn parse_ipv6_nameserver() {
        let raw = r#"
resolver #1
  nameserver[0] : 2001:4860:4860::8888
  flags    : Request AAAA records
"#;
        let (resolvers, source) = parse_scutil_dns(raw);
        assert_eq!(resolvers, vec!["2001:4860:4860::8888".to_string()]);
        assert_eq!(source, "scutil-all");
    }

    #[test]
    fn parse_empty_fixture() {
        let (resolvers, _) = parse_scutil_dns("DNS configuration\n");
        assert!(resolvers.is_empty());
    }

    #[test]
    fn truncate_hint_is_char_boundary_safe() {
        // Multibyte-heavy text must not panic on the 400-cut.
        let raw: String = "解析器测试é".repeat(120);
        let h = truncate_hint(&raw, 400);
        assert!(
            h.contains('…'),
            "should be truncated, got {} chars",
            h.chars().count()
        );
        assert!(h.ends_with("bytes)"));
        assert!(h.chars().count() >= 400);

        let short = "a".repeat(10);
        assert_eq!(truncate_hint(&short, 400), short);

        let exact = "b".repeat(400);
        assert_eq!(truncate_hint(&exact, 400), exact);
    }
}
