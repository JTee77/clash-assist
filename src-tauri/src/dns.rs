//! System DNS resolver listing.
//! - macOS: `scutil --dns`; prefer scoped tunnel/VPN resolvers when present, otherwise unique nameservers.
//! - Windows: adapter APIs (`dns_windows.rs` 采集，`select_windows_resolvers` 纯函数选择，windows-plan §2.3)。

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
            blocks
                .iter()
                .filter(|b| !b.nameservers.is_empty())
                .collect()
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

// ---------------------------------------------------------------------------
// Windows：网卡 DNS 选择（windows-plan §2.3，M2）
//
// FFI（GetAdaptersAddresses / GetBestRoute2）在 `dns_windows.rs`，只采集事实；
// 下面的选择逻辑是纯函数，所有平台单测。
//
// 这是「近似」：网卡上配置的 DNS 不一定就是系统实际使用的解析器。
// - SMHNR：Windows 默认会同时向多张网卡的 DNS 发查询；
// - NRPT：VPN / DirectAccess 下发的按域名指定解析器的策略会覆盖网卡 DNS；
// - 系统 DoH（Win11 的「加密 DNS」）会把查询改走 HTTPS。
// 判定以 whoami 实测为准，这里只决定「system」那一路问哪个解析器（PR #51 评审 S-1）。
// ---------------------------------------------------------------------------

/// `IF_TYPE_SOFTWARE_LOOPBACK`（ipifcons.h）。
pub const IF_TYPE_SOFTWARE_LOOPBACK: u32 = 24;
/// `IF_TYPE_PROP_VIRTUAL`：Wintun 网卡报的类型（待 V7 真机确认）。
pub const IF_TYPE_PROP_VIRTUAL: u32 = 53;
/// `IF_TYPE_TUNNEL`。
pub const IF_TYPE_TUNNEL: u32 = 131;

/// Verge 在 Windows 上 `tun.device` 的默认值（yaml 未配置时用它识别 TUN 网卡）。
pub const VERGE_DEFAULT_TUN_DEVICE: &str = "Mihomo";
/// mihomo 内核在 `device` 未配置时的默认网卡名（PR #51 Q1：需要第二个证据才认）。
pub const MIHOMO_DEFAULT_TUN_DEVICE: &str = "Meta";

/// 一张网卡的 DNS 相关事实（由 `dns_windows::adapters` 从 `IP_ADAPTER_ADDRESSES_LH` 映射）。
#[cfg_attr(not(windows), allow(dead_code))] // 只在 Windows 采集；单测全平台跑
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdapterDns {
    /// IPv4 ifIndex；只有 IPv6 的网卡取 `Ipv6IfIndex`。
    pub if_index: u32,
    pub friendly_name: String,
    pub description: String,
    pub if_type: u32,
    /// `OperStatus == IfOperStatusUp`。
    pub oper_up: bool,
    /// 按网卡上的配置顺序（首选在前）。
    pub dns: Vec<std::net::IpAddr>,
    /// `Flags & IP_ADAPTER_IPV4_ENABLED`。
    pub ipv4_enabled: bool,
    pub ipv4_metric: u32,
    pub ipv6_metric: u32,
}

impl AdapterDns {
    /// 「全部网卡」排序用的跃点数（PR #51 S-2）：未启用 IPv4 的网卡 `Ipv4Metric` 可能是 0，
    /// 改用 `Ipv6Metric`；两者都没有时排到最后。
    fn sort_metric(&self) -> u32 {
        if self.ipv4_enabled {
            self.ipv4_metric
        } else if self.ipv6_metric != 0 {
            self.ipv6_metric
        } else {
            u32::MAX
        }
    }
}

/// Windows 在没有 IPv6 DNS 的网卡上填的站点本地占位地址 `fec0:0:0:ffff::1/2/3`。
fn is_win_placeholder_dns(ip: &std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V6(v6) => {
            let s = v6.segments();
            s[..7] == [0xfec0, 0, 0, 0xffff, 0, 0, 0] && (1..=3).contains(&s[7])
        }
        std::net::IpAddr::V4(_) => false,
    }
}

fn is_usable_dns(ip: &std::net::IpAddr) -> bool {
    !ip.is_unspecified() && !is_win_placeholder_dns(ip)
}

/// IPv6 链路本地（fe80::/10）。不带 zone id 无法直接查询，排在最后，
/// `find_system_resolver` 也不选它。
fn is_ipv6_link_local(ip: &std::net::IpAddr) -> bool {
    matches!(ip, std::net::IpAddr::V6(v6) if (v6.segments()[0] & 0xffc0) == 0xfe80)
}

/// 排序键：IPv4 → IPv6 → IPv6 链路本地（§2.3 第 5 步「IPv4 在前」）。
fn resolver_rank(ip: &std::net::IpAddr) -> u8 {
    match ip {
        std::net::IpAddr::V4(_) => 0,
        _ if is_ipv6_link_local(ip) => 2,
        std::net::IpAddr::V6(_) => 1,
    }
}

/// 字符串形式的解析器地址是否可以直接用于 `dns_query`（排除 IPv6 链路本地）。
#[cfg_attr(not(windows), allow(dead_code))]
fn is_queryable_resolver(s: &str) -> bool {
    s.parse::<std::net::IpAddr>()
        .map(|ip| !is_ipv6_link_local(&ip))
        .unwrap_or(false)
}

/// 识别 TUN 网卡时命中的规则（写进 raw_hint，PR #51 M-2(a)）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TunRule {
    /// 网卡名等于 yaml 的 `tun.device`。
    YamlDevice,
    /// `tun.device` 未配置，网卡名等于 Verge 默认名 `Mihomo`。
    VergeDefaultName,
    /// `tun.device` 未配置，网卡名等于 mihomo 默认名 `Meta`，且类型为虚拟 / 隧道或描述含 Wintun。
    MihomoDefaultNameWithSignal,
    /// 描述含 `Wintun`。WireGuard 等 VPN 也用 Wintun，所以只能说明是「TUN 或 VPN」。
    WintunDescription,
}

impl TunRule {
    pub fn label(self) -> &'static str {
        match self {
            TunRule::YamlDevice => "网卡名等于 yaml 的 tun.device",
            TunRule::VergeDefaultName => "tun.device 未配置，网卡名等于 Verge 默认名 Mihomo",
            TunRule::MihomoDefaultNameWithSignal => {
                "tun.device 未配置，网卡名等于 mihomo 默认名 Meta，且为虚拟/隧道网卡或描述含 Wintun"
            }
            TunRule::WintunDescription => {
                "描述含 Wintun（TUN 或 VPN：WireGuard 等也用 Wintun，不一定是 Mihomo）"
            }
        }
    }
}

/// 识别 Verge / mihomo 的 TUN 网卡（§2.3 第 2 步；PR #51 Q1）。
///
/// - 配置了 `tun.device`：网卡名等于它；
/// - 未配置（或为空）：网卡名等于 `Mihomo`；或等于 `Meta` 且有第二个证据
///   （IfType 为虚拟 / 隧道，或描述含 Wintun）；
/// - 任何情况下：描述含 `Wintun`（V7 待验证；注意它也会命中 WireGuard）。
///
/// Windows 网卡名不区分大小写，按 ASCII 忽略大小写比较。
fn tun_rule(a: &AdapterDns, tun_hint: Option<&str>) -> Option<TunRule> {
    let name = a.friendly_name.trim();
    let wintun = a.description.to_ascii_lowercase().contains("wintun");
    match tun_hint.map(str::trim).filter(|s| !s.is_empty()) {
        Some(h) => {
            if name.eq_ignore_ascii_case(h) {
                return Some(TunRule::YamlDevice);
            }
        }
        None => {
            if name.eq_ignore_ascii_case(VERGE_DEFAULT_TUN_DEVICE) {
                return Some(TunRule::VergeDefaultName);
            }
            let virtual_type = matches!(a.if_type, IF_TYPE_PROP_VIRTUAL | IF_TYPE_TUNNEL);
            if name.eq_ignore_ascii_case(MIHOMO_DEFAULT_TUN_DEVICE) && (virtual_type || wintun) {
                return Some(TunRule::MihomoDefaultNameWithSignal);
            }
        }
    }
    wintun.then_some(TunRule::WintunDescription)
}

/// `pick_windows_adapters` 的结果：选中的网卡、source，以及写进 raw_hint 的理由与警告。
#[derive(Debug, Clone)]
pub struct WinPick<'a> {
    pub adapters: Vec<&'a AdapterDns>,
    pub source: &'static str,
    /// 选择理由（PR #51 M-2）。
    pub reasons: Vec<String>,
    /// 需要用户注意的情况（如 TUN 不是默认路由网卡）。目前只写进 raw_hint。
    // TODO(后续 PR，与 M3 前端一起)：把 warnings 做成 DnsResolversResult 的结构化字段，
    // 前端在 TUN 非默认路由且 whoami 判「未泄漏」时把 DNS 卡降为 warn（PR #51 r2 建议）。
    pub warnings: Vec<String>,
}

/// 按 §2.3 选出用于取 DNS 的网卡。
///
/// 1. 过滤：未启用、回环（IfType 24）、去掉占位地址后没有 DNS 的网卡；
/// 2. TUN 优先 → `win-adapter-tun`；
/// 3. 否则默认路由网卡 → `win-adapter-default`；
/// 4. 否则全部（按跃点数、ifIndex 排序）→ `win-adapter-all`；
/// 5. 一张都没有 → `win-adapter-none`。
#[cfg_attr(not(windows), allow(dead_code))]
pub fn pick_windows_adapters<'a>(
    adapters: &'a [AdapterDns],
    tun_hint: Option<&str>,
    default_if: Option<u32>,
) -> WinPick<'a> {
    let mut eligible: Vec<&AdapterDns> = adapters
        .iter()
        .filter(|a| a.oper_up && a.if_type != IF_TYPE_SOFTWARE_LOOPBACK)
        .filter(|a| a.dns.iter().any(is_usable_dns))
        .collect();
    // 稳定排序：跃点数小的在前，便于 `win-adapter-all` 时「第一个」更接近系统首选。
    eligible.sort_by_key(|a| (a.sort_metric(), a.if_index));

    let mut reasons = Vec::new();
    let mut warnings = Vec::new();

    let tun: Vec<(&AdapterDns, TunRule)> = eligible
        .iter()
        .filter_map(|a| tun_rule(a, tun_hint).map(|r| (*a, r)))
        .collect();
    if !tun.is_empty() {
        for (a, rule) in &tun {
            reasons.push(format!(
                "TUN 网卡 {}：命中规则「{}」",
                fmt_adapter_short(a),
                rule.label()
            ));
        }
        match default_if {
            Some(d) if !tun.iter().any(|(a, _)| a.if_index == d) => warnings.push(format!(
                "选中的 TUN 网卡不是默认路由网卡（ifIndex {d}），TUN 可能没有承载流量（例如未开启 auto-route）。\
此时查询 TUN 上的 DNS（常见为 198.18.x 的 fake-ip）只会测到 mihomo 自己的出口，可能把真实的泄漏误判为未泄漏"
            )),
            Some(_) => reasons.push("TUN 网卡同时是默认路由网卡".to_string()),
            None => warnings.push(
                "默认路由网卡未知，无法确认 TUN 是否在承载流量".to_string(),
            ),
        }
        return WinPick {
            adapters: tun.into_iter().map(|(a, _)| a).collect(),
            source: "win-adapter-tun",
            reasons,
            warnings,
        };
    }

    if let Some(idx) = default_if {
        let def: Vec<&AdapterDns> = eligible
            .iter()
            .copied()
            .filter(|a| a.if_index == idx)
            .collect();
        if !def.is_empty() {
            reasons.push(format!(
                "未识别到 TUN 网卡；使用默认路由网卡 {}",
                fmt_adapter_short(def[0])
            ));
            return WinPick {
                adapters: def,
                source: "win-adapter-default",
                reasons,
                warnings,
            };
        }
    }

    if !eligible.is_empty() {
        reasons.push(match default_if {
            Some(d) => {
                let why = match adapters.iter().find(|a| a.if_index == d) {
                    Some(a) if !a.oper_up => "未启用",
                    Some(a) if a.if_type == IF_TYPE_SOFTWARE_LOOPBACK => "是回环网卡",
                    Some(_) => "没有可用 DNS",
                    None => "不在网卡列表中",
                };
                format!(
                    "未识别到 TUN 网卡；默认路由网卡（ifIndex {d}）{why}，已退回全部网卡（按跃点数排序）"
                )
            }
            None => "未识别到 TUN 网卡；默认路由网卡未知，已退回全部网卡（按跃点数排序）"
                .to_string(),
        });
        return WinPick {
            adapters: eligible,
            source: "win-adapter-all",
            reasons,
            warnings,
        };
    }

    reasons.push("没有已启用且配置了 DNS 的网卡".to_string());
    WinPick {
        adapters: Vec::new(),
        source: "win-adapter-none",
        reasons,
        warnings,
    }
}

/// windows-plan §2.3 的纯函数：从网卡列表中选出系统 DNS 解析器。
///
/// 返回（解析器，source）。解析器去重、保留网卡上的首选顺序，再稳定排序为
/// IPv4 → IPv6 → IPv6 链路本地；source 恒为 `win-adapter-*`。
// 运行时由 `windows_resolvers_result` 直接用 `pick_windows_adapters`（需要理由与警告）；
// 这个精简入口保留给 §2.3 的全平台单测。
#[cfg_attr(not(test), allow(dead_code))]
pub fn select_windows_resolvers(
    adapters: &[AdapterDns],
    tun_hint: Option<&str>,
    default_if: Option<u32>,
) -> (Vec<String>, &'static str) {
    let pick = pick_windows_adapters(adapters, tun_hint, default_if);
    (resolvers_of(&pick.adapters), pick.source)
}

fn resolvers_of(picked: &[&AdapterDns]) -> Vec<String> {
    let mut ips: Vec<std::net::IpAddr> = Vec::new();
    for a in picked {
        for ip in a.dns.iter().filter(|ip| is_usable_dns(ip)) {
            if !ips.contains(ip) {
                ips.push(*ip);
            }
        }
    }
    ips.sort_by_key(resolver_rank);
    ips.into_iter().map(|ip| ip.to_string()).collect()
}

/// 从 Verge yaml 文本中取 `tun.device`（块映射或单行流式映射）。
///
/// 只认顶层 `tun:` 下第一层缩进的 `device:`；值去引号、去行尾注释，空值返回 None。
/// 手写解析（仓库没有 yaml crate，PR #51 Q4 接受）：先去掉 BOM；未加引号的值以
/// `*` `&` `!` `[` `{` `|` `>` 开头（别名、锚点、标签、流式集合、多行标量）时返回 None，
/// 不把它们当成网卡名。
#[cfg_attr(not(windows), allow(dead_code))]
pub fn parse_tun_device(content: &str) -> Option<String> {
    fn clean(raw: &str) -> Option<String> {
        let v = raw.trim();
        let v = match v.chars().next() {
            // 加引号：取到配对的引号为止（之后的注释忽略）。
            Some(q @ ('"' | '\'')) => {
                let inner = &v[1..];
                &inner[..inner.find(q)?]
            }
            Some('#' | '*' | '&' | '!' | '[' | '{' | '|' | '>') => return None,
            // 未加引号：` #` 之后是行尾注释。
            _ => v.find(" #").map_or(v, |i| &v[..i]),
        };
        let v = v.trim();
        (!v.is_empty()).then(|| v.to_string())
    }

    let content = content.strip_prefix('\u{feff}').unwrap_or(content);
    let mut lines = content.lines();
    while let Some(line) = lines.next() {
        let Some(rest) = line.strip_prefix("tun:") else {
            continue;
        };
        let rest = rest.trim();
        // 流式：`tun: { enable: true, device: Mihomo }`
        if let Some(body) = rest.strip_prefix('{') {
            let body = body.split('}').next().unwrap_or("");
            for kv in body.split(',') {
                if let Some((k, v)) = kv.split_once(':') {
                    if k.trim() == "device" {
                        return clean(v);
                    }
                }
            }
            return None;
        }
        if !(rest.is_empty() || rest.starts_with('#')) {
            return None;
        }
        // 块映射：第一层缩进由第一个非空、非注释的子行决定。
        let mut child_indent: Option<usize> = None;
        for l in lines.by_ref() {
            let t = l.trim_start();
            if t.is_empty() || t.starts_with('#') {
                continue;
            }
            let indent = l.len() - t.len();
            if indent == 0 {
                break;
            }
            let ci = *child_indent.get_or_insert(indent);
            if indent != ci {
                continue;
            }
            if let Some(v) = t.strip_prefix("device:") {
                return clean(v);
            }
        }
        return None;
    }
    None
}

/// Windows 侧采集到的事实（`dns_windows::collect`），交给纯函数组装结果。
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Debug, Clone, Default)]
pub struct WinDnsFacts {
    pub adapters: Vec<AdapterDns>,
    /// `GetBestRoute2` 查到的默认路由网卡 ifIndex。
    pub default_if: Option<u32>,
    /// 默认路由查询失败时的说明（只进 raw_hint）。
    pub default_route_note: Option<String>,
    /// Verge yaml 的 `tun.device`。
    pub tun_hint: Option<String>,
    /// `tun_hint` 取自哪个文件（只进 raw_hint）。
    pub tun_hint_from: Option<String>,
}

/// 近似性说明（§2.3 SMHNR 备注；PR #51 S-1 补充 NRPT 与系统 DoH），只在诊断详情里提一句。
pub const WIN_APPROX_NOTE: &str = "说明：这里列的是网卡上配置的 DNS，只是近似。\
Windows 默认启用 SMHNR，会同时向多张网卡的 DNS 发查询，而 whoami 只测所选的这一个解析器，\
TUN 模式且未开启 strict-route 时可能低估泄漏；VPN / DirectAccess 下发的按域名解析策略（NRPT）\
和系统 DoH 也会覆盖网卡 DNS。以 whoami 实测为准。";

const WIN_HINT_MAX_ADAPTERS: usize = 12;

fn fmt_adapter_short(a: &AdapterDns) -> String {
    format!("{} (ifIndex {})", a.friendly_name, a.if_index)
}

/// 由采集到的事实组装 `list_dns_resolvers_blocking` 的 Windows 结果（纯函数）。
///
/// 隐私（PR #51 S-3）：raw_hint 会列出网卡名、描述和局域网 / 公司内网的 DNS 地址，
/// 与 macOS 上 scutil 原文同类，不是 secret。目前它只显示在本机诊断卡片里，
/// 没有导出或分享入口（Windows 首版也没有 CLI），所以不做脱敏；
/// 以后若增加导出 / 分享，需要先对私有地址脱敏。
#[cfg_attr(not(windows), allow(dead_code))]
pub fn windows_resolvers_result(f: &WinDnsFacts) -> DnsResolversResult {
    let pick = pick_windows_adapters(&f.adapters, f.tun_hint.as_deref(), f.default_if);
    let mut resolvers = resolvers_of(&pick.adapters);

    // PR #51 M-3：只有 IPv6 链路本地 DNS 时，列表与 find_system_resolver 要一致——
    // 都视为「没有可查询的解析器」，并写明原因。
    // TODO(zone-id)：支持链路本地 DNS 需要从 sin6_scope_id 取 scope 并一路传到
    // dns_query（IpAddr 带不了 zone），放到后续 PR。
    let mut link_local_only: Option<String> = None;
    if !resolvers.is_empty() && !resolvers.iter().any(|s| is_queryable_resolver(s)) {
        link_local_only = Some(resolvers.join(", "));
        resolvers.clear();
    }

    let mut hint: Vec<String> = Vec::new();
    hint.push(format!(
        "选中网卡（{}）：{}",
        pick.source,
        if pick.adapters.is_empty() {
            "无".to_string()
        } else {
            pick.adapters
                .iter()
                .map(|a| fmt_adapter_short(a))
                .collect::<Vec<_>>()
                .join("、")
        }
    ));
    for r in &pick.reasons {
        hint.push(format!("选择理由：{r}"));
    }
    for w in &pick.warnings {
        hint.push(format!("⚠ 警告：{w}"));
    }
    if let Some(ll) = &link_local_only {
        hint.push(format!(
            "⚠ 仅有 IPv6 链路本地 DNS（{ll}），查询需要 zone id，本版不查询"
        ));
    }
    hint.push(match (f.default_if, &f.default_route_note) {
        (Some(i), _) => format!("默认路由网卡 ifIndex：{i}"),
        (None, Some(n)) => format!("默认路由网卡：未知（{n}）"),
        (None, None) => "默认路由网卡：未知".to_string(),
    });
    hint.push(match (&f.tun_hint, &f.tun_hint_from) {
        (Some(d), Some(from)) => format!("tun.device：{d}（{from}）"),
        (Some(d), None) => format!("tun.device：{d}"),
        (None, _) => format!(
            "tun.device：未配置，按默认名 {VERGE_DEFAULT_TUN_DEVICE}（或 {MIHOMO_DEFAULT_TUN_DEVICE} 加第二个证据）"
        ),
    });
    hint.push(WIN_APPROX_NOTE.to_string());
    hint.push(format!("网卡（共 {} 张）：", f.adapters.len()));
    hint.extend(f.adapters.iter().take(WIN_HINT_MAX_ADAPTERS).map(|a| {
        let dns: Vec<String> = a.dns.iter().map(|ip| ip.to_string()).collect();
        format!(
            "- {} #{} [{}] type {} {} metric v4 {} / v6 {} dns {}",
            a.friendly_name,
            a.if_index,
            a.description,
            a.if_type,
            if a.oper_up { "up" } else { "down" },
            if a.ipv4_enabled {
                a.ipv4_metric.to_string()
            } else {
                "-".to_string()
            },
            a.ipv6_metric,
            if dns.is_empty() {
                "无".to_string()
            } else {
                dns.join(",")
            }
        )
    }));
    if f.adapters.len() > WIN_HINT_MAX_ADAPTERS {
        hint.push(format!(
            "- …其余 {} 张未列出",
            f.adapters.len() - WIN_HINT_MAX_ADAPTERS
        ));
    }

    let error = if let Some(ll) = &link_local_only {
        Some(format!(
            "无法确定系统 DNS 解析器：仅有 IPv6 链路本地 DNS（{ll}），需要 zone id，本版不查询"
        ))
    } else if resolvers.is_empty() {
        Some("无法确定系统 DNS 解析器：已启用的网卡上没有找到 DNS 服务器".to_string())
    } else {
        None
    };
    DnsResolversResult {
        resolvers,
        source: pick.source.to_string(),
        raw_hint: Some(truncate_hint(&hint.join("\n"), 2400)),
        error,
    }
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
    // Windows：GetAdaptersAddresses + GetBestRoute2（§2.3，M2）。
    #[cfg(windows)]
    {
        match crate::dns_windows::collect() {
            Ok(facts) => windows_resolvers_result(&facts),
            Err(e) => DnsResolversResult {
                resolvers: vec![],
                source: "win-adapter-error".into(),
                raw_hint: None,
                error: Some(format!("读取网卡 DNS 失败：{e}")),
            },
        }
    }

    #[cfg(not(any(target_os = "macos", windows)))]
    {
        DnsResolversResult {
            resolvers: vec![],
            source: "unsupported-platform".into(),
            raw_hint: None,
            error: Some(
                "DNS 解析器列表仅支持 macOS（scutil --dns）与 Windows（网卡 API）；当前平台不可用。"
                    .into(),
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
/// - Windows: 网卡 API 选出的第一个可直接查询的解析器（§2.3：TUN 网卡 → 默认路由网卡 →
///   全部；跳过 IPv6 链路本地）。拿不到时返回 `None`，调用方报「无法确定系统 DNS 解析器」，
///   **不再回落到 8.8.8.8**（R6）。
fn find_system_resolver() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        let r = list_dns_resolvers_blocking();
        r.resolvers.into_iter().next()
    }
    #[cfg(target_os = "linux")]
    {
        #[allow(
            clippy::disallowed_methods,
            reason = "仅 Linux/Unix，不在 S-R1 的 Windows 风险面"
        )]
        let resolv = std::fs::read_to_string("/etc/resolv.conf");
        if let Ok(content) = resolv {
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
        let r = list_dns_resolvers_blocking();
        r.resolvers.into_iter().find(|s| is_queryable_resolver(s))
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
            None => {
                return whoami_err(
                    "system".to_string(),
                    "无法确定系统 DNS 解析器地址".to_string(),
                )
            }
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
        let raw =
            "\"ns\" \"172.68.41.102\"\n\"ip\" \"146.19.163.158\"\n\"ecs\" \"146.19.163.0/24/24\"\n";
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

    // ---- Windows 网卡 DNS 选择（§2.3，M2；全平台跑）----

    use std::net::IpAddr;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    fn adapter(idx: u32, name: &str, desc: &str, dns: &[&str]) -> AdapterDns {
        AdapterDns {
            if_index: idx,
            friendly_name: name.to_string(),
            description: desc.to_string(),
            if_type: 6, // IF_TYPE_ETHERNET_CSMACD
            oper_up: true,
            dns: dns.iter().map(|s| ip(s)).collect(),
            ipv4_enabled: true,
            ipv4_metric: 25,
            ipv6_metric: 25,
        }
    }

    fn wifi() -> AdapterDns {
        let mut a = adapter(
            12,
            "Wi-Fi",
            "Intel(R) Wi-Fi 6 AX201 160MHz",
            &["fec0:0:0:ffff::1", "192.168.1.1", "fec0:0:0:ffff::2"],
        );
        a.if_type = 71; // IF_TYPE_IEEE80211
        a
    }

    fn ethernet() -> AdapterDns {
        let mut a = adapter(7, "以太网", "Realtek PCIe GbE", &["10.0.0.1", "10.0.0.2"]);
        a.ipv4_metric = 5;
        a
    }

    fn mihomo_tun() -> AdapterDns {
        let mut a = adapter(23, "Mihomo", "Mihomo Tunnel", &["198.18.0.2"]);
        a.if_type = 53; // IF_TYPE_PROP_VIRTUAL
        a.ipv4_metric = 0;
        a
    }

    fn loopback() -> AdapterDns {
        let mut a = adapter(
            1,
            "Loopback Pseudo-Interface 1",
            "Software Loopback Interface 1",
            &["127.0.0.1"],
        );
        a.if_type = IF_TYPE_SOFTWARE_LOOPBACK;
        a
    }

    #[test]
    fn win_select_tun_by_default_name() {
        let ads = vec![wifi(), mihomo_tun(), loopback()];
        let (r, src) = select_windows_resolvers(&ads, None, Some(12));
        assert_eq!(src, "win-adapter-tun");
        assert_eq!(r, vec!["198.18.0.2".to_string()]);
    }

    #[test]
    fn win_select_tun_by_yaml_device_case_insensitive() {
        let mut tun = mihomo_tun();
        tun.friendly_name = "ClashTun".into();
        tun.description = "Some Tunnel".into();
        let ads = vec![wifi(), tun.clone()];
        let (r, src) = select_windows_resolvers(&ads, Some("clashtun"), Some(12));
        assert_eq!(src, "win-adapter-tun");
        assert_eq!(r, vec!["198.18.0.2".to_string()]);

        // 配置了别的名字时，默认名 Mihomo 不再算 TUN。
        let ads = vec![wifi(), mihomo_tun()];
        let (r, src) = select_windows_resolvers(&ads, Some("ClashTun"), Some(12));
        assert_eq!(src, "win-adapter-default");
        assert_eq!(r, vec!["192.168.1.1".to_string()]);
    }

    #[test]
    fn win_select_tun_by_wintun_description() {
        let mut tun = mihomo_tun();
        tun.friendly_name = "本地连接* 9".into();
        tun.description = "WireGuard Tunnel / Wintun Userspace Tunnel".into();
        let ads = vec![wifi(), tun];
        let (_, src) = select_windows_resolvers(&ads, Some("Mihomo"), Some(12));
        assert_eq!(src, "win-adapter-tun");
    }

    #[test]
    fn win_blank_tun_hint_falls_back_to_default_name() {
        let ads = vec![wifi(), mihomo_tun()];
        let (_, src) = select_windows_resolvers(&ads, Some("  "), Some(12));
        assert_eq!(src, "win-adapter-tun");
    }

    #[test]
    fn win_tun_down_or_without_dns_is_skipped() {
        let mut down = mihomo_tun();
        down.oper_up = false;
        let (r, src) = select_windows_resolvers(&[wifi(), down], None, Some(12));
        assert_eq!(src, "win-adapter-default");
        assert_eq!(r, vec!["192.168.1.1".to_string()]);

        let mut nodns = mihomo_tun();
        nodns.dns = vec![ip("fec0:0:0:ffff::1"), ip("0.0.0.0")];
        let (_, src) = select_windows_resolvers(&[wifi(), nodns], None, Some(12));
        assert_eq!(src, "win-adapter-default");
    }

    #[test]
    fn win_select_default_route_adapter() {
        let ads = vec![ethernet(), wifi(), loopback()];
        let (r, src) = select_windows_resolvers(&ads, None, Some(12));
        assert_eq!(src, "win-adapter-default");
        // 占位 fec0:0:0:ffff::1/2 被剔除。
        assert_eq!(r, vec!["192.168.1.1".to_string()]);
    }

    #[test]
    fn win_default_route_on_loopback_or_unknown_falls_back_to_all() {
        let ads = vec![wifi(), ethernet(), loopback()];
        // 默认路由指向回环（被过滤）→ 全部。
        let (r, src) = select_windows_resolvers(&ads, None, Some(1));
        assert_eq!(src, "win-adapter-all");
        // 跃点数小的以太网（5）在 Wi-Fi（25）前；回环不参与。
        assert_eq!(
            r,
            vec![
                "10.0.0.1".to_string(),
                "10.0.0.2".to_string(),
                "192.168.1.1".to_string()
            ]
        );
        let (r2, src2) = select_windows_resolvers(&ads, None, None);
        assert_eq!(src2, "win-adapter-all");
        assert_eq!(r2, r);
    }

    #[test]
    fn win_none_when_no_usable_adapter() {
        let mut down = wifi();
        down.oper_up = false;
        let (r, src) = select_windows_resolvers(&[down, loopback()], None, Some(12));
        assert!(r.is_empty());
        assert_eq!(src, "win-adapter-none");
        let (r, src) = select_windows_resolvers(&[], None, None);
        assert!(r.is_empty());
        assert_eq!(src, "win-adapter-none");
    }

    #[test]
    fn win_ipv4_first_then_ipv6_then_link_local_dedup() {
        let a = adapter(
            12,
            "Wi-Fi",
            "x",
            &[
                "fe80::1",
                "2001:4860:4860::8888",
                "192.168.1.1",
                "fec0:0:0:ffff::3",
                "192.168.1.1",
                "8.8.4.4",
            ],
        );
        let (r, _) = select_windows_resolvers(&[a], None, Some(12));
        assert_eq!(
            r,
            vec![
                "192.168.1.1".to_string(),
                "8.8.4.4".to_string(),
                "2001:4860:4860::8888".to_string(),
                "fe80::1".to_string(),
            ]
        );
    }

    #[test]
    fn win_placeholder_only_matches_ffff_1_to_3() {
        assert!(is_win_placeholder_dns(&ip("fec0:0:0:ffff::1")));
        assert!(is_win_placeholder_dns(&ip("fec0:0:0:ffff::3")));
        assert!(!is_win_placeholder_dns(&ip("fec0:0:0:ffff::4")));
        assert!(!is_win_placeholder_dns(&ip("fec0::1")));
        assert!(!is_win_placeholder_dns(&ip("192.168.1.1")));
    }

    #[test]
    fn win_queryable_resolver_skips_link_local() {
        assert!(is_queryable_resolver("192.168.1.1"));
        assert!(is_queryable_resolver("2001:4860:4860::8888"));
        assert!(!is_queryable_resolver("fe80::1"));
        assert!(!is_queryable_resolver("febf::1"));
        assert!(is_queryable_resolver("fec0::1"));
        assert!(!is_queryable_resolver("bogus"));
    }

    #[test]
    fn win_multiple_tun_adapters_are_merged() {
        let mut t2 = mihomo_tun();
        t2.if_index = 30;
        t2.friendly_name = "Other".into();
        t2.description = "Wintun Userspace Tunnel".into();
        t2.dns = vec![ip("198.18.0.2"), ip("198.18.0.3")];
        let (r, src) = select_windows_resolvers(&[mihomo_tun(), t2], None, None);
        assert_eq!(src, "win-adapter-tun");
        assert_eq!(r, vec!["198.18.0.2".to_string(), "198.18.0.3".to_string()]);
    }

    #[test]
    fn win_result_hint_and_error() {
        let facts = WinDnsFacts {
            adapters: vec![wifi(), mihomo_tun(), loopback()],
            default_if: Some(12),
            default_route_note: None,
            tun_hint: Some("Mihomo".into()),
            tun_hint_from: Some("clash-verge.yaml".into()),
        };
        let r = windows_resolvers_result(&facts);
        assert_eq!(r.source, "win-adapter-tun");
        assert_eq!(r.resolvers, vec!["198.18.0.2".to_string()]);
        assert!(r.error.is_none());
        let hint = r.raw_hint.unwrap();
        assert!(hint.contains("Mihomo (ifIndex 23)"), "{hint}");
        assert!(hint.contains("默认路由网卡 ifIndex：12"), "{hint}");
        assert!(hint.contains("clash-verge.yaml"), "{hint}");
        assert!(hint.contains("SMHNR"), "{hint}");

        let empty = WinDnsFacts {
            adapters: vec![loopback()],
            default_if: None,
            default_route_note: Some("GetBestRoute2 失败（Win32 1232）".into()),
            ..Default::default()
        };
        let r = windows_resolvers_result(&empty);
        assert_eq!(r.source, "win-adapter-none");
        assert!(r.resolvers.is_empty());
        assert!(r.error.unwrap().contains("无法确定系统 DNS 解析器"));
        let hint = r.raw_hint.unwrap();
        assert!(hint.contains("Win32 1232"), "{hint}");
        assert!(hint.contains("按默认名 Mihomo"), "{hint}");
    }

    #[test]
    fn win_resolvers_never_invent_public_dns() {
        // R6：结果只能来自网卡上的配置，绝不凭空出现 8.8.8.8。
        let ads = vec![wifi(), ethernet(), mihomo_tun()];
        for hint in [None, Some("Mihomo"), Some("x")] {
            for def in [None, Some(7), Some(12), Some(99)] {
                let (r, src) = select_windows_resolvers(&ads, hint, def);
                assert!(src.starts_with("win-adapter-"));
                assert!(!r.iter().any(|s| s == "8.8.8.8"), "{r:?}");
            }
        }
    }

    fn hint_of(facts: &WinDnsFacts) -> String {
        windows_resolvers_result(facts).raw_hint.unwrap()
    }

    #[test]
    fn win_meta_needs_unset_hint_and_second_signal() {
        let mut meta = mihomo_tun();
        meta.friendly_name = "Meta".into();
        meta.description = "Meta Tunnel".into();
        // 虚拟网卡类型（53）+ 未配置 tun.device → 认。
        let p = pick_windows_adapters(std::slice::from_ref(&meta), None, Some(23));
        assert_eq!(p.source, "win-adapter-tun");
        assert!(p.reasons[0].contains("Meta"), "{:?}", p.reasons);
        assert_eq!(
            tun_rule(&meta, None),
            Some(TunRule::MihomoDefaultNameWithSignal)
        );
        // 隧道类型（131）也算第二个证据。
        let mut tunnel = meta.clone();
        tunnel.if_type = IF_TYPE_TUNNEL;
        assert_eq!(
            tun_rule(&tunnel, None),
            Some(TunRule::MihomoDefaultNameWithSignal)
        );
        // 没有第二个证据（以太网类型、描述不含 Wintun）→ 不认。
        let mut plain = meta.clone();
        plain.if_type = 6;
        assert_eq!(tun_rule(&plain, None), None);
        // 描述含 Wintun 也算第二个证据。
        plain.description = "Wintun Userspace Tunnel".into();
        assert_eq!(
            tun_rule(&plain, None),
            Some(TunRule::MihomoDefaultNameWithSignal)
        );
        // 配置了别的 tun.device → Meta 不再按名字认（虚拟类型也不行）。
        assert_eq!(tun_rule(&meta, Some("Mihomo")), None);
    }

    #[test]
    fn win_wintun_rule_is_labelled_tun_or_vpn() {
        let mut wg = mihomo_tun();
        wg.friendly_name = "wg0".into();
        wg.description = "WireGuard Tunnel / Wintun Userspace Tunnel".into();
        assert_eq!(tun_rule(&wg, None), Some(TunRule::WintunDescription));
        let p = pick_windows_adapters(std::slice::from_ref(&wg), None, Some(23));
        assert!(p.reasons[0].contains("TUN 或 VPN"), "{:?}", p.reasons);
    }

    #[test]
    fn win_hint_states_matched_rule() {
        let facts = WinDnsFacts {
            adapters: vec![wifi(), mihomo_tun()],
            default_if: Some(23),
            ..Default::default()
        };
        let h = hint_of(&facts);
        assert!(h.contains("选择理由：TUN 网卡 Mihomo (ifIndex 23)"), "{h}");
        assert!(h.contains("Verge 默认名 Mihomo"), "{h}");
        assert!(h.contains("TUN 网卡同时是默认路由网卡"), "{h}");
        assert!(!h.contains("警告"), "{h}");

        let facts = WinDnsFacts {
            adapters: vec![wifi(), mihomo_tun()],
            default_if: Some(23),
            tun_hint: Some("Mihomo".into()),
            tun_hint_from: Some("clash-verge.yaml".into()),
            ..Default::default()
        };
        let h = hint_of(&facts);
        assert!(h.contains("网卡名等于 yaml 的 tun.device"), "{h}");
    }

    #[test]
    fn win_warns_when_tun_is_not_default_route() {
        let facts = WinDnsFacts {
            adapters: vec![wifi(), mihomo_tun()],
            default_if: Some(12),
            ..Default::default()
        };
        let r = windows_resolvers_result(&facts);
        assert_eq!(r.source, "win-adapter-tun");
        let h = r.raw_hint.unwrap();
        assert!(
            h.contains("⚠ 警告：选中的 TUN 网卡不是默认路由网卡（ifIndex 12）"),
            "{h}"
        );
        assert!(h.contains("198.18"), "{h}");

        // 默认路由未知：提示无法确认。
        let facts = WinDnsFacts {
            adapters: vec![wifi(), mihomo_tun()],
            default_if: None,
            ..Default::default()
        };
        let h = hint_of(&facts);
        assert!(h.contains("无法确认 TUN 是否在承载流量"), "{h}");
    }

    #[test]
    fn win_hint_states_fallback_to_all() {
        // 默认路由网卡（Wi-Fi，12）只有占位 DNS → 退回全部网卡，并写明原因。
        let mut w = wifi();
        w.dns = vec![ip("fec0:0:0:ffff::1")];
        let facts = WinDnsFacts {
            adapters: vec![w, ethernet()],
            default_if: Some(12),
            ..Default::default()
        };
        let r = windows_resolvers_result(&facts);
        assert_eq!(r.source, "win-adapter-all");
        let h = r.raw_hint.unwrap();
        assert!(
            h.contains("默认路由网卡（ifIndex 12）没有可用 DNS，已退回全部网卡"),
            "{h}"
        );

        // 默认路由网卡未启用 / 是回环 / 未知。
        let mut down = wifi();
        down.oper_up = false;
        let ads = [down, ethernet()];
        let p = pick_windows_adapters(&ads, None, Some(12));
        assert!(p.reasons[0].contains("未启用"), "{:?}", p.reasons);
        let ads = [loopback(), ethernet()];
        let p = pick_windows_adapters(&ads, None, Some(1));
        assert!(p.reasons[0].contains("回环"), "{:?}", p.reasons);
        let ads = [ethernet()];
        let p = pick_windows_adapters(&ads, None, None);
        assert!(p.reasons[0].contains("默认路由网卡未知"), "{:?}", p.reasons);
    }

    #[test]
    fn win_link_local_only_returns_no_resolver_with_reason() {
        let mut w = wifi();
        w.dns = vec![ip("fec0:0:0:ffff::1"), ip("fe80::1"), ip("fe80::2")];
        let facts = WinDnsFacts {
            adapters: vec![w],
            default_if: Some(12),
            ..Default::default()
        };
        let r = windows_resolvers_result(&facts);
        assert_eq!(r.source, "win-adapter-default");
        assert!(r.resolvers.is_empty(), "{:?}", r.resolvers);
        let e = r.error.unwrap();
        assert!(e.contains("仅有 IPv6 链路本地 DNS"), "{e}");
        assert!(e.contains("fe80::1, fe80::2"), "{e}");
        assert!(e.contains("zone id"), "{e}");
        assert!(r.raw_hint.unwrap().contains("本版不查询"));

        // 链路本地与可查询地址并存时照常返回，链路本地排最后。
        let mut w = wifi();
        w.dns = vec![ip("fe80::1"), ip("2001:db8::53")];
        let facts = WinDnsFacts {
            adapters: vec![w],
            default_if: Some(12),
            ..Default::default()
        };
        let r = windows_resolvers_result(&facts);
        assert!(r.error.is_none());
        assert_eq!(
            r.resolvers,
            vec!["2001:db8::53".to_string(), "fe80::1".to_string()]
        );
    }

    #[test]
    fn win_all_sort_uses_ipv6_metric_for_ipv6_only_adapter() {
        // 只有 IPv6 的网卡 Ipv4Metric 为 0，不能因此排到最前（PR #51 S-2）。
        let mut v6only = adapter(40, "v6only", "x", &["2001:db8::53"]);
        v6only.ipv4_enabled = false;
        v6only.ipv4_metric = 0;
        v6only.ipv6_metric = 50;
        let (r, src) = select_windows_resolvers(&[v6only.clone(), ethernet()], None, None);
        assert_eq!(src, "win-adapter-all");
        let ads = [v6only.clone(), ethernet()];
        let p = pick_windows_adapters(&ads, None, None);
        assert_eq!(p.adapters[0].if_index, 7, "以太网（metric 5）应在前");
        assert_eq!(r[0], "10.0.0.1");
        // 两个 metric 都没有 → 排最后。
        v6only.ipv6_metric = 0;
        let mut eth = ethernet();
        eth.ipv4_metric = 9_999;
        let ads = [v6only, eth];
        let p = pick_windows_adapters(&ads, None, None);
        assert_eq!(p.adapters[0].if_index, 7);
    }

    #[test]
    fn win_hint_mentions_nrpt_and_doh() {
        let h = hint_of(&WinDnsFacts {
            adapters: vec![wifi()],
            default_if: Some(12),
            ..Default::default()
        });
        assert!(
            h.contains("NRPT") && h.contains("DoH") && h.contains("SMHNR"),
            "{h}"
        );
    }

    // ---- Verge yaml 的 tun.device ----

    #[test]
    fn tun_device_block_mapping() {
        let y = "mixed-port: 7897\ntun:\n  enable: true\n  stack: gvisor\n  device: Mihomo\n  dns-hijack:\n    - any:53\nsecret: x\n";
        assert_eq!(parse_tun_device(y).as_deref(), Some("Mihomo"));
    }

    #[test]
    fn tun_device_quoted_and_comment() {
        assert_eq!(
            parse_tun_device("tun:\n  device: 'My Tun' # comment\n").as_deref(),
            Some("My Tun")
        );
        assert_eq!(
            parse_tun_device("tun: # x\n\n  # c\n  device: \"Meta\"\n").as_deref(),
            Some("Meta")
        );
        assert_eq!(
            parse_tun_device("tun:\n  device: Mihomo # 注释\n").as_deref(),
            Some("Mihomo")
        );
    }

    #[test]
    fn tun_device_ignores_nested_and_other_sections() {
        // 嵌套更深的 device 不算；dns 段下的 device 不算。
        let y = "dns:\n  device: nope\ntun:\n  enable: true\n  route:\n    device: deep\nprofile:\n  device: other\n";
        assert_eq!(parse_tun_device(y), None);
        // tun 段之后的顶层 device 不算。
        assert_eq!(
            parse_tun_device("tun:\n  enable: true\ndevice: top\n"),
            None
        );
        // 空值。
        assert_eq!(parse_tun_device("tun:\n  device: ''\n"), None);
        assert_eq!(parse_tun_device("tun:\n  device:\n"), None);
        // 没有 tun 段。
        assert_eq!(parse_tun_device("mixed-port: 7897\n"), None);
        // 缩进的 `tun:`（不在顶层）不算。
        assert_eq!(parse_tun_device("x:\n  tun:\n    device: a\n"), None);
    }

    #[test]
    fn tun_device_strips_bom_and_rejects_yaml_specials() {
        assert_eq!(
            parse_tun_device("\u{feff}tun:\n  device: Mihomo\n").as_deref(),
            Some("Mihomo")
        );
        for v in [
            "*anchor",
            "&a Mihomo",
            "!!str Mihomo",
            "[a, b]",
            "{x: y}",
            "|",
            ">",
        ] {
            let y = format!("tun:\n  device: {v}\n");
            assert_eq!(parse_tun_device(&y), None, "{v}");
        }
        assert_eq!(parse_tun_device("tun: { device: *x }\n"), None);
        // 加了引号的就是字面量。
        assert_eq!(
            parse_tun_device("tun:\n  device: '*x'\n").as_deref(),
            Some("*x")
        );
    }

    #[test]
    fn tun_device_flow_mapping_and_crlf() {
        assert_eq!(
            parse_tun_device("tun: { enable: true, device: Mihomo }\n").as_deref(),
            Some("Mihomo")
        );
        assert_eq!(parse_tun_device("tun: {enable: true}\n"), None);
        assert_eq!(
            parse_tun_device("tun:\r\n  enable: true\r\n  device: Mihomo\r\n").as_deref(),
            Some("Mihomo")
        );
    }

    // ---- Windows runner 上的实机断言（windows-plan §6 M2 行）----

    #[cfg(windows)]
    #[test]
    fn win_runner_source_is_adapter_and_not_hardcoded() {
        let r = list_dns_resolvers_blocking();
        eprintln!(
            "list_dns_resolvers_blocking: source={} resolvers={:?} error={:?}\n{}",
            r.source,
            r.resolvers,
            r.error,
            r.raw_hint.as_deref().unwrap_or("")
        );
        assert!(r.source.starts_with("win-adapter-"), "{}", r.source);
        assert_ne!(r.source, "win-adapter-error", "{:?}", r.error);
        assert_ne!(r.source, "win-adapter-none", "runner 上应能找到网卡 DNS");
        assert!(!r.resolvers.is_empty());

        let sys = find_system_resolver();
        let sys = sys.expect("find_system_resolver 在 runner 上应返回网卡 DNS");
        // 必须来自网卡列表（不是写死的地址）。
        assert!(r.resolvers.contains(&sys), "{sys} 不在 {:?}", r.resolvers);
        if !r.resolvers.iter().any(|s| s == "8.8.8.8") {
            assert_ne!(sys, "8.8.8.8");
        }
    }

    /// 与 `Get-DnsClientServerAddress` 对照选中网卡的 DNS（真机清单 6 的自动化版本）。
    #[cfg(windows)]
    #[test]
    fn win_runner_matches_get_dnsclientserveraddress() {
        let facts = crate::dns_windows::collect().expect("collect");
        let pick =
            pick_windows_adapters(&facts.adapters, facts.tun_hint.as_deref(), facts.default_if);
        let picked = pick.adapters;
        eprintln!(
            "source={} default_if={:?} reasons={:?} warnings={:?}",
            pick.source, facts.default_if, pick.reasons, pick.warnings
        );
        for a in &facts.adapters {
            eprintln!("{a:?}");
        }
        let Some(a) = picked.first() else {
            panic!("runner 上应选中至少一张网卡");
        };
        let script = format!(
            "(Get-DnsClientServerAddress -InterfaceIndex {} -ErrorAction Stop).ServerAddresses",
            a.if_index
        );
        let out = match std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .output()
        {
            Ok(o) if o.status.success() => o,
            other => {
                // PR #51 M-1：CI 上不允许静默跳过（通过的测试不显示 stderr，看不出是否真的对照过）。
                if std::env::var_os("CI").is_some() {
                    panic!("CI 上 PowerShell 对照必须执行，但调用失败：{other:?}");
                }
                eprintln!("跳过（非 CI）：PowerShell 不可用 {other:?}");
                return;
            }
        };
        let ps: std::collections::BTreeSet<IpAddr> = String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter_map(|l| l.trim().parse().ok())
            .filter(is_usable_dns)
            .collect();
        let ours: std::collections::BTreeSet<IpAddr> =
            a.dns.iter().copied().filter(is_usable_dns).collect();
        assert_eq!(
            ours, ps,
            "ifIndex {} 的 DNS 与 PowerShell 不一致",
            a.if_index
        );
        // PR #51 r2 建议：把「真的比对过」留在 CI 日志里。通过的测试的 stdout 会被 cargo
        // 吞掉，所以有 GITHUB_STEP_SUMMARY 时同时写进 job summary。
        let marker = format!(
            "[win-dns-crosscheck] 已执行：{} ifIndex {} 的 DNS {:?} 与 Get-DnsClientServerAddress 一致",
            pick.source, a.if_index, ours
        );
        println!("{marker}");
        if let Some(path) = std::env::var_os("GITHUB_STEP_SUMMARY") {
            use std::io::Write as _;
            let mut f = crate::safe_fs::open_append(std::path::Path::new(&path))
                .expect("打开 GITHUB_STEP_SUMMARY");
            writeln!(f, "- {marker}").expect("写 GITHUB_STEP_SUMMARY");
        }
    }
}
