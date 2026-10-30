//! Clash Verge Rev / Mihomo controller + per-client discovery.
//!
//! Resolves the external-controller host/port/secret/mixed-port and the local
//! Unix-socket path for a given proxy client. Verge tries multiple sock
//! candidates (service sock on 2.5.6+, legacy `/tmp/verge/...`, `$TMPDIR`, yaml
//! `external-controller-unix`, optional `-ext-ctl-unix` cmdline). Other clients
//! read the per-OS table in `crate::platform` and never receive Verge's socket.

use crate::platform;
use regex::Regex;
use std::path::{Path, PathBuf};

#[cfg(unix)]
use super::transport::http_via_unix;
use super::types::{DiscoverResult, DEFAULT_MIXED, DEFAULT_PORT};

/// 原始 Verge `config.yaml`（含 secret）。Windows 上不编译：不把 secret 交给前端。
#[cfg(not(windows))]
#[allow(clippy::disallowed_methods, reason = "仅非 Windows 编译（cfg(not(windows))），不在 S-R1 的 Windows 风险面")]
pub fn read_verge_config() -> Result<String, String> {
    let path = platform::verge_config_path();
    std::fs::read_to_string(&path).map_err(|e| format!("read {}: {}", path.display(), e))
}

/// Expand `~` / `$TMPDIR` / `${TMPDIR}` in a path string.
pub fn expand_path_str(path: &str) -> PathBuf {
    let mut s = path.trim().to_string();
    if s.is_empty() {
        return PathBuf::new();
    }
    if let Ok(tmpdir) = std::env::var("TMPDIR") {
        let tmp = tmpdir.trim_end_matches('/');
        s = s.replace("${TMPDIR}", tmp).replace("$TMPDIR", tmp);
    } else {
        s = s.replace("${TMPDIR}", "/tmp").replace("$TMPDIR", "/tmp");
    }
    expand_home(&s)
}

fn expand_home(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        return dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("/"))
            .join(rest);
    }
    if path == "~" {
        return dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
    }
    PathBuf::from(path)
}

/// Parsed controller fields from Clash/Mihomo yaml.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ParsedControllerYaml {
    /// TCP EC port when `external-controller: host:port` is present and non-blank.
    pub port: Option<u16>,
    pub secret: String,
    pub mixed_port: Option<u16>,
    /// Raw `external-controller-unix` value (may contain `$TMPDIR` / `~`).
    pub unix_path: Option<String>,
    /// True when `external-controller:` exists but is blank (`''` / `""` / empty).
    pub tcp_ec_blank: bool,
}

/// Parse external-controller / unix / secret / mixed-port from yaml text.
/// Blank `external-controller: ''` does **not** yield a port (callers must not
/// pretend DEFAULT_PORT came from config).
pub fn parse_controller_yaml_rich(content: &str) -> ParsedControllerYaml {
    let mut out = ParsedControllerYaml::default();

    // TCP EC host:port — require colon+digits; do not match *-unix / *-cors keys.
    if let Ok(re) = Regex::new(
        r#"(?m)^external-controller:\s*['"]?([^:'"\s#]+):(\d+)['"]?\s*(?:#.*)?$"#,
    ) {
        if let Some(c) = re.captures(content) {
            if let Ok(p) = c[2].parse() {
                out.port = Some(p);
            }
        }
    }

    // Blank EC: external-controller: '' | "" | <empty>
    if out.port.is_none() {
        if let Ok(re) =
            Regex::new(r#"(?m)^external-controller:\s*(?:''|""|)\s*(?:#.*)?$"#)
        {
            if re.is_match(content) {
                out.tcp_ec_blank = true;
            }
        }
    }

    if let Ok(re) =
        Regex::new(r#"(?m)^external-controller-unix:\s*['"]?([^\s'"]+)['"]?"#)
    {
        if let Some(c) = re.captures(content) {
            let raw = c[1].trim();
            if !raw.is_empty() {
                out.unix_path = Some(raw.to_string());
            }
        }
    }

    if let Ok(re) = Regex::new(r#"(?m)^secret:\s*['"]?([^\s'"]+)['"]?"#) {
        if let Some(c) = re.captures(content) {
            out.secret = c[1].to_string();
        }
    }

    if let Ok(re) = Regex::new(r"(?m)^mixed-port:\s*(\d+)") {
        if let Some(c) = re.captures(content) {
            if let Ok(p) = c[1].parse() {
                out.mixed_port = Some(p);
            }
        }
    }

    out
}

/// Backward-compatible triple parse (port falls back to `default_port` when absent).
pub fn parse_controller_yaml(
    content: &str,
    default_port: u16,
    default_mixed: u16,
) -> (u16, String, u16) {
    let rich = parse_controller_yaml_rich(content);
    (
        rich.port.unwrap_or(default_port),
        rich.secret,
        rich.mixed_port.unwrap_or(default_mixed),
    )
}

fn merge_parsed(into: &mut ParsedControllerYaml, other: ParsedControllerYaml) {
    // Prefer earlier file's EC decision. clash-verge.yaml (read first) may set
    // tcp_ec_blank; do not let stale config.yaml host:port override that.
    if into.tcp_ec_blank {
        // keep port None
    } else if into.port.is_none() {
        into.port = other.port;
        if into.port.is_none() && other.tcp_ec_blank {
            into.tcp_ec_blank = true;
        }
    }
    if into.port.is_some() {
        into.tcp_ec_blank = false;
    }
    if into.secret.is_empty() && !other.secret.is_empty() {
        into.secret = other.secret;
    }
    if into.mixed_port.is_none() {
        into.mixed_port = other.mixed_port;
    }
    if into.unix_path.is_none() {
        into.unix_path = other.unix_path;
    }
}

/// Best-effort: parse `-ext-ctl-unix <path>` from a running verge-mihomo cmdline.
#[cfg(unix)]
fn sock_from_verge_cmdline() -> Option<String> {
    let output = std::process::Command::new("ps")
        .args(["-axo", "args="])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let re = Regex::new(r"-ext-ctl-unix\s+(\S+)").ok()?;
    for line in text.lines() {
        if !(line.contains("verge-mihomo") || line.contains("clash-verge")) {
            continue;
        }
        if let Some(c) = re.captures(line) {
            let p = c[1].trim().trim_matches('"').trim_matches('\'');
            if !p.is_empty() {
                return Some(p.to_string());
            }
        }
    }
    None
}

#[cfg(not(unix))]
fn sock_from_verge_cmdline() -> Option<String> {
    None
}

/// Probe sock with GET /version (short timeout). Returns true on HTTP 2xx.
#[cfg(unix)]
fn sock_answers_version(sock: &str, secret: &str) -> bool {
    #[allow(clippy::disallowed_methods, reason = "仅 Unix，不在 S-R1 的 Windows 风险面")]
    let sock_exists = Path::new(sock).exists();
    if !sock_exists {
        return false;
    }
    match http_via_unix("GET", "/version", None, secret, Some(sock), 800) {
        Ok(res) if (200..300).contains(&res.status) => true,
        Ok(res) if res.status == 401 || res.status == 403 => {
            // Auth required but sock is alive — still usable with the right secret.
            true
        }
        _ => {
            // Retry once with empty secret (Verge service sock often has no auth).
            if !secret.is_empty() {
                matches!(
                    http_via_unix("GET", "/version", None, "", Some(sock), 800),
                    Ok(res)
                        if (200..300).contains(&res.status)
                            || res.status == 401
                            || res.status == 403
                )
            } else {
                false
            }
        }
    }
}

/// Pick first trusted-owner candidate that exists and answers /version; else first existing trusted one; else None.
#[cfg(unix)]
pub fn select_working_sock(candidates: &[String], secret: &str) -> Option<String> {
    let mut first_existing: Option<String> = None;
    for cand in candidates {
        let p = expand_path_str(cand);
        if p.as_os_str().is_empty() {
            continue;
        }
        let s = p.to_string_lossy().into_owned();
        #[allow(clippy::disallowed_methods, reason = "仅 Unix，不在 S-R1 的 Windows 风险面")]
        let sock_exists = Path::new(&s).exists();
        if !sock_exists {
            continue;
        }
        // 0.2.9：其他普通用户拥有的套接字不作候选（连回落候选也不当）。
        if crate::platform::ensure_sock_path_trusted(&s).is_err() {
            continue;
        }
        if first_existing.is_none() {
            first_existing = Some(s.clone());
        }
        if sock_answers_version(&s, secret) {
            return Some(s);
        }
    }
    first_existing
}

/// 非 Unix 平台（Windows）：没有 Unix 套接字，也没有路径信任校验
/// （`platform::ensure_sock_path_trusted` 只在 Unix 编译）。所有候选一律判为不可用，
/// 不做任何探测、不发送 secret——绝不在缺少信任校验时静默放行。
/// Windows 的本机 IPC 由 M1 的命名管道路径（`pipe::open_verified`）接管。
#[cfg(not(unix))]
pub fn select_working_sock(_candidates: &[String], _secret: &str) -> Option<String> {
    None
}

fn read_and_parse(path: &Path) -> Option<ParsedControllerYaml> {
    #[allow(clippy::disallowed_methods, reason = "TODO(M1): 仅读配置文件路径，非管道候选（S-R1）")]
    let content = std::fs::read_to_string(path).ok()?;
    Some(parse_controller_yaml_rich(&content))
}

/// Discover Verge controller: merge stale `config.yaml` + fresher `clash-verge.yaml`,
/// then resolve a working unix sock across service / legacy / TMPDIR / yaml / cmdline.
pub fn discover_controller() -> DiscoverResult {
    withhold_secret_on_windows(discover_controller_inner())
}

/// Windows：发现结果里**不携带 secret**（方案 §2.1 规则 4，M-3 / C-1 / C-4）。
///
/// Windows 上还没有 `verify_tcp_listener`，任何 TCP 监听者都只能算
/// `listener-unverified`（服务模式恒为 unverified，C-4），而 unverified 一律不发送 secret。
/// 所以 secret 在离开发现阶段之前就被清空：不交给前端（`discover_mihomo*` 命令），
/// 也不会被前端回传进 `mihomo_http` / `list_nodes`。TCP 出口本身另有 fail-closed 门控
/// （`transport::http_via_tcp_async`）。
///
/// TODO(M1) verify_tcp_listener: 只有 listener-verified 时才保留 secret。
#[cfg(windows)]
fn withhold_secret_on_windows(mut d: DiscoverResult) -> DiscoverResult {
    d.secret = String::new();
    d
}

#[cfg(not(windows))]
#[inline]
fn withhold_secret_on_windows(d: DiscoverResult) -> DiscoverResult {
    d
}

fn discover_controller_inner() -> DiscoverResult {
    let config_path = platform::verge_config_path();
    let clash_verge_path = platform::verge_clash_verge_yaml_path();

    let mut parsed = ParsedControllerYaml::default();
    let mut source_files: Vec<String> = Vec::new();

    // Prefer clash-verge.yaml first (live EC fields), then config.yaml (often stale).
    for path in [&clash_verge_path, &config_path] {
        if let Some(p) = read_and_parse(path) {
            source_files.push(path.display().to_string());
            merge_parsed(&mut parsed, p);
        }
    }

    let yaml_unix: Vec<String> = parsed
        .unix_path
        .as_ref()
        .map(|u| {
            let expanded = expand_path_str(u);
            let s = expanded.to_string_lossy().into_owned();
            if s.is_empty() {
                vec![]
            } else {
                vec![s]
            }
        })
        .unwrap_or_default();

    let cmdline_sock = sock_from_verge_cmdline();
    let extras: Vec<String> = cmdline_sock.into_iter().collect();
    let candidates = platform::verge_sock_candidates(&yaml_unix, &extras);
    let sock_path = select_working_sock(&candidates, &parsed.secret);

    let mixed = parsed.mixed_port.unwrap_or(DEFAULT_MIXED);
    let secret = parsed.secret;

    // TCP port: only trust yaml when host:port was explicitly present.
    // Blank EC + working sock → keep DEFAULT_PORT as soft UI fallback but mark source.
    let (port, tcp_from_yaml) = match parsed.port {
        Some(p) => (p, true),
        None => (DEFAULT_PORT, false),
    };

    let source = match (&sock_path, tcp_from_yaml, source_files.is_empty(), parsed.tcp_ec_blank) {
        (Some(sock), true, false, _) => {
            format!("verge-config+unix:{}|{}", source_files.join(","), sock)
        }
        (Some(sock), false, false, _) => {
            format!("verge-unix:{}|yaml:{}", sock, source_files.join(","))
        }
        (Some(sock), _, true, _) => format!("verge-unix:{sock}"),
        (None, true, false, _) => format!("verge-config:{}", source_files.join(",")),
        (None, false, false, true) => {
            format!("verge-ec-blank:{}", source_files.join(","))
        }
        (None, false, false, false) => format!("verge-config:{}", source_files.join(",")),
        (None, _, true, _) => "defaults".into(),
    };

    DiscoverResult {
        host: "127.0.0.1".into(),
        port,
        secret,
        mixed_port: mixed,
        source,
        sock_path,
    }
}

/// Read first readable yaml among `paths`; set sock_path only from sock_candidates that exist.
/// Never injects Verge DEFAULT_SOCK unless that path is explicitly in sock_candidates.
pub fn read_controller_yaml(
    paths: &[String],
    default_port: u16,
    default_mixed: u16,
    sock_candidates: &[String],
    source_prefix: &str,
) -> DiscoverResult {
    // Unix 套接字候选：只在 Unix 上做存在性检查。非 Unix（Windows）上一律为 None，
    // 绝不对候选调用 `Path::exists`——候选若是管道名，`exists` 会以默认 QoS 打开管道（M-2）。
    // Windows 的管道候选由 M1 的 `pipe::open_verified` 处理。
    #[cfg(unix)]
    #[allow(clippy::disallowed_methods, reason = "仅 Unix，不在 S-R1 的 Windows 风险面")]
    let sock_path: Option<String> = sock_candidates.iter().find_map(|cand| {
        let p = expand_path_str(cand);
        if !p.as_os_str().is_empty() && p.exists() {
            Some(p.to_string_lossy().into_owned())
        } else {
            None
        }
    });
    #[cfg(not(unix))]
    let sock_path: Option<String> = {
        let _ = sock_candidates;
        None
    };

    for raw in paths {
        let path = expand_path_str(raw);
        #[allow(clippy::disallowed_methods, reason = "TODO(M1): 仅读配置文件路径，非管道候选（S-R1）")]
        let exists = path.exists();
        if !exists {
            continue;
        }
        #[allow(clippy::disallowed_methods, reason = "TODO(M1): 仅读配置文件路径，非管道候选（S-R1）")]
        let read = std::fs::read_to_string(&path);
        if let Ok(content) = read {
            let (port, secret, mixed) =
                parse_controller_yaml(&content, default_port, default_mixed);
            return DiscoverResult {
                host: "127.0.0.1".into(),
                port,
                secret,
                mixed_port: mixed,
                source: format!("{source_prefix}:{}", path.display()),
                sock_path,
            };
        }
    }

    // Also try scanning directories for any *.yaml / *.yml (FlClash / Nyanpasu)
    for raw in paths {
        let path = expand_path_str(raw);
        #[allow(clippy::disallowed_methods, reason = "TODO(M1): 仅读配置文件路径，非管道候选（S-R1）")]
        let is_dir = path.is_dir();
        if is_dir {
            if let Ok(entries) = std::fs::read_dir(&path) {
                let mut yamls: Vec<PathBuf> = entries
                    .filter_map(|e| e.ok().map(|e| e.path()))
                    .filter(|p| {
                        p.extension()
                            .and_then(|x| x.to_str())
                            .map(|ext| ext == "yaml" || ext == "yml")
                            .unwrap_or(false)
                    })
                    .collect();
                yamls.sort();
                for y in yamls {
                    #[allow(clippy::disallowed_methods, reason = "TODO(M1): 仅读配置文件路径，非管道候选（S-R1）")]
                    let read = std::fs::read_to_string(&y);
                    if let Ok(content) = read {
                        let (port, secret, mixed) =
                            parse_controller_yaml(&content, default_port, default_mixed);
                        return DiscoverResult {
                            host: "127.0.0.1".into(),
                            port,
                            secret,
                            mixed_port: mixed,
                            source: format!("{source_prefix}:{}", y.display()),
                            sock_path: sock_path.clone(),
                        };
                    }
                }
            }
        }
    }

    DiscoverResult {
        host: "127.0.0.1".into(),
        port: default_port,
        secret: String::new(),
        mixed_port: default_mixed,
        source: format!("{source_prefix}:defaults"),
        sock_path,
    }
}

/// Client-scoped discovery. Verge goes through `discover_controller`; other
/// clients read the per-OS path table in `crate::platform` (macOS keeps the
/// historical hardcoded paths; non-Verge clients never get Verge's sock).
/// Parse FlClash controller fields from a decoded JSON object (`flutter.config`,
/// `flutter.clash_config`, or top-level `shared_preferences.json`).
pub fn parse_flclash_json_str(raw_json: &str) -> Option<ParsedControllerYaml> {
    let val: serde_json::Value = serde_json::from_str(raw_json).ok()?;
    let obj = val.as_object()?;

    // Could be a top-level shared_preferences.json containing `"flutter.config": "{...}"`
    if let Some(nested_str) = obj
        .get("flutter.config")
        .or_else(|| obj.get("config"))
        .and_then(|v| v.as_str())
    {
        if let Some(parsed) = parse_flclash_json_str(nested_str) {
            return Some(parsed);
        }
    }
    if let Some(nested_str) = obj
        .get("flutter.clash_config")
        .or_else(|| obj.get("clash_config"))
        .and_then(|v| v.as_str())
    {
        if let Some(parsed) = parse_flclash_json_str(nested_str) {
            return Some(parsed);
        }
    }

    // Or the decoded Config / PatchClashConfig object itself
    let patch = obj
        .get("patchClashConfig")
        .and_then(|v| v.as_object())
        .unwrap_or(obj);

    let has_any_flclash_key = patch.contains_key("external-controller")
        || patch.contains_key("externalController")
        || patch.contains_key("mixed-port")
        || patch.contains_key("mixedPort")
        || patch.contains_key("secret");
    if !has_any_flclash_key {
        return None;
    }

    let mut out = ParsedControllerYaml::default();

    if let Some(ec_val) = patch
        .get("external-controller")
        .or_else(|| patch.get("externalController"))
    {
        if let Some(ec_str) = ec_val.as_str() {
            let trimmed = ec_str.trim();
            if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("close") {
                out.tcp_ec_blank = true;
            } else if trimmed.eq_ignore_ascii_case("open") {
                out.port = Some(9090);
            } else if let Some((_, port_str)) = trimmed.rsplit_once(':') {
                if let Ok(p) = port_str.trim().parse::<u16>() {
                    out.port = Some(p);
                }
            }
        } else if let Some(b) = ec_val.as_bool() {
            if b {
                out.port = Some(9090);
            } else {
                out.tcp_ec_blank = true;
            }
        }
    }

    if let Some(sec) = patch.get("secret").and_then(|v| v.as_str()) {
        out.secret = sec.trim().to_string();
    }

    if let Some(mp) = patch
        .get("mixed-port")
        .or_else(|| patch.get("mixedPort"))
        .and_then(|v| v.as_u64())
    {
        if let Ok(p) = u16::try_from(mp) {
            if p > 0 {
                out.mixed_port = Some(p);
            }
        }
    }

    Some(out)
}

/// Read a FlClash preference file (`.plist` via `plutil -convert json -o -` on macOS,
/// or direct `.json`) and extract `ParsedControllerYaml`.
fn read_flclash_pref_file(path: &Path) -> Option<ParsedControllerYaml> {
    #[allow(clippy::disallowed_methods, reason = "TODO(M1): 仅读配置文件路径，非管道候选（S-R1）")]
    let is_existing_file = path.exists() && path.is_file();
    if !is_existing_file {
        return None;
    }
    #[cfg_attr(not(target_os = "macos"), allow(unused_variables))] // 只有 macOS 分支读 plist
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    #[cfg(target_os = "macos")]
    if ext == "plist" {
        let output = std::process::Command::new("/usr/bin/plutil")
            .args(["-convert", "json", "-o", "-"])
            .arg(path)
            .output()
            .ok()?;
        if output.status.success() {
            let text = String::from_utf8_lossy(&output.stdout);
            if let Some(p) = parse_flclash_json_str(&text) {
                return Some(p);
            }
        }
    }

    #[allow(clippy::disallowed_methods, reason = "TODO(M1): 仅读配置文件路径，非管道候选（S-R1）")]
    let content = std::fs::read_to_string(path).ok()?;
    parse_flclash_json_str(&content)
}

/// Discover FlClash controller from preferences (`.plist` / `shared_preferences.json`)
/// and runtime `config.yaml` files.
pub fn discover_flclash_with_paths(
    pref_paths: &[String],
    yaml_paths: &[String],
    default_port: u16,
    default_mixed: u16,
) -> DiscoverResult {
    let mut parsed = ParsedControllerYaml::default();
    let mut sources: Vec<String> = Vec::new();

    for raw in pref_paths {
        let path = expand_path_str(raw);
        if let Some(p) = read_flclash_pref_file(&path) {
            sources.push(path.display().to_string());
            merge_parsed(&mut parsed, p);
        }
    }

    for raw in yaml_paths {
        let path = expand_path_str(raw);
        #[allow(clippy::disallowed_methods, reason = "TODO(M1): 仅读配置文件路径，非管道候选（S-R1）")]
        let is_file = path.is_file();
        // 与原来的 `if is_file { … } else if path.is_dir() { … }` 等价：只有不是文件时才查 is_dir
        #[allow(clippy::disallowed_methods, reason = "TODO(M1): 仅读配置文件路径，非管道候选（S-R1）")]
        let is_dir = !is_file && path.is_dir();
        if is_file {
            if let Some(p) = read_and_parse(&path) {
                sources.push(path.display().to_string());
                merge_parsed(&mut parsed, p);
            }
        } else if is_dir {
            let cfg_yaml = path.join("config.yaml");
            #[allow(clippy::disallowed_methods, reason = "TODO(M1): 仅读配置文件路径，非管道候选（S-R1）")]
            let cfg_is_file = cfg_yaml.is_file();
            if cfg_is_file {
                let disp = cfg_yaml.display().to_string();
                if !sources.contains(&disp) {
                    if let Some(p) = read_and_parse(&cfg_yaml) {
                        sources.push(disp);
                        merge_parsed(&mut parsed, p);
                    }
                }
            }
        }
    }

    let port = parsed.port.unwrap_or(default_port);
    let mixed = parsed.mixed_port.unwrap_or(default_mixed);
    let secret = parsed.secret;

    let source = if sources.is_empty() {
        "flclash:defaults".to_string()
    } else if parsed.tcp_ec_blank && parsed.port.is_none() {
        format!("flclash-ec-disabled:{}", sources.join(","))
    } else {
        format!("flclash:{}", sources.join(","))
    };

    DiscoverResult {
        host: "127.0.0.1".into(),
        port,
        secret,
        mixed_port: mixed,
        source,
        sock_path: None,
    }
}

pub fn discover_for_client(client_id: &str) -> DiscoverResult {
    withhold_secret_on_windows(discover_for_client_inner(client_id))
}

fn discover_for_client_inner(client_id: &str) -> DiscoverResult {
    if client_id == "verge" {
        return discover_controller();
    }
    if client_id == "flclash" {
        let spec = platform::client_discovery("flclash");
        let (yaml_paths, default_port, default_mixed) = match spec {
            Some(s) => (s.yaml_paths, s.default_port, s.default_mixed),
            None => (vec![], 9090, 7890),
        };
        let pref_paths = platform::flclash_preference_paths();
        return discover_flclash_with_paths(&pref_paths, &yaml_paths, default_port, default_mixed);
    }
    if let Some(spec) = platform::client_discovery(client_id) {
        return read_controller_yaml(
            &spec.yaml_paths,
            spec.default_port,
            spec.default_mixed,
            &spec.sock_candidates,
            spec.source_prefix,
        );
    }
    DiscoverResult {
        host: "127.0.0.1".into(),
        port: 9090,
        secret: String::new(),
        mixed_port: 7890,
        source: format!("unknown-client:{client_id}"),
        sock_path: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_controller_yaml_reads_sample() {
        let sample = r#"
mixed-port: 7890
external-controller: 127.0.0.1:9090
secret: "test-secret"
"#;
        let (port, secret, mixed) = parse_controller_yaml(sample, 1, 2);
        assert_eq!(port, 9090);
        assert_eq!(secret, "test-secret");
        assert_eq!(mixed, 7890);
    }

    #[test]
    fn parse_rich_reads_unix_and_blank_ec() {
        let sample = r#"
mixed-port: 7897
external-controller: ''
secret: set-your-secret
external-controller-unix: $TMPDIR/verge-mihomo.sock
"#;
        let rich = parse_controller_yaml_rich(sample);
        assert_eq!(rich.port, None);
        assert!(rich.tcp_ec_blank);
        assert_eq!(
            rich.unix_path.as_deref(),
            Some("$TMPDIR/verge-mihomo.sock")
        );
        assert_eq!(rich.secret, "set-your-secret");
        assert_eq!(rich.mixed_port, Some(7897));
    }

    #[test]
    fn parse_rich_does_not_confuse_cors_or_unix_keys() {
        let sample = r#"
external-controller-unix: /tmp/verge/verge-mihomo.sock
external-controller-cors:
  allow-private-network: true
external-controller: 127.0.0.1:9097
"#;
        let rich = parse_controller_yaml_rich(sample);
        assert_eq!(rich.port, Some(9097));
        assert_eq!(
            rich.unix_path.as_deref(),
            Some("/tmp/verge/verge-mihomo.sock")
        );
        assert!(!rich.tcp_ec_blank);
    }

    #[test]
    fn parse_rich_stale_config_with_tcp_still_parses() {
        let sample = r#"
external-controller: 127.0.0.1:9097
external-controller-unix: /tmp/verge/verge-mihomo.sock
secret: set-your-secret
mixed-port: 7897
"#;
        let rich = parse_controller_yaml_rich(sample);
        assert_eq!(rich.port, Some(9097));
        assert_eq!(
            rich.unix_path.as_deref(),
            Some("/tmp/verge/verge-mihomo.sock")
        );
    }

    #[test]
    fn expand_path_str_tmpdir_and_home() {
        let prev = std::env::var("TMPDIR").ok();
        std::env::set_var("TMPDIR", "/tmp/clash-assist-test-tmpdir");
        let p = expand_path_str("$TMPDIR/verge-mihomo.sock");
        if let Some(v) = prev {
            std::env::set_var("TMPDIR", v);
        } else {
            std::env::set_var("TMPDIR", "/tmp");
        }
        assert_eq!(
            p.to_string_lossy(),
            "/tmp/clash-assist-test-tmpdir/verge-mihomo.sock"
        );
        let home = dirs::home_dir().expect("home");
        let p2 = expand_path_str("~/Library/foo.sock");
        assert_eq!(p2, home.join("Library/foo.sock"));
    }

    #[test]
    fn select_working_sock_prefers_existing() {
        let missing = format!("/tmp/clash-assist-no-sock-{}.sock", std::process::id());
        let _ = std::fs::remove_file(&missing);
        // No live Verge required: all missing → None
        let r = select_working_sock(std::slice::from_ref(&missing), "");
        assert!(r.is_none());
    }

    #[test]
    fn verge_candidates_include_service_and_legacy() {
        #[cfg_attr(not(unix), allow(unused_variables))]
        let c = platform::verge_sock_candidates(&[], &[]);
        #[cfg(unix)]
        {
            assert!(
                c.iter()
                    .any(|p| p.contains("clash-verge-service") && p.ends_with("verge-mihomo.sock")),
                "service sock missing: {c:?}"
            );
            assert!(c.iter().any(|p| p == platform::VERGE_SOCK_PATH));
        }
    }

    #[test]
    fn non_verge_discover_never_returns_verge_sock_when_absent() {
        // Ensure Verge DEFAULT_SOCK is not present or we still must not inject it for other clients.
        let clashx = discover_for_client("clashx_meta");
        assert!(
            clashx.sock_path.is_none(),
            "clashx_meta sock must be None (never Verge), got {:?}",
            clashx.sock_path
        );

        let fl = discover_for_client("flclash");
        assert!(
            fl.sock_path.is_none(),
            "flclash sock must be None, got {:?}",
            fl.sock_path
        );

        let ny = discover_for_client("nyanpasu");
        assert!(
            ny.sock_path.is_none(),
            "nyanpasu sock must be None, got {:?}",
            ny.sock_path
        );

        let party = discover_for_client("mihomo_party");
        if let Some(ref s) = party.sock_path {
            assert_ne!(
                s,
                platform::VERGE_SOCK_PATH,
                "party must never use Verge sock"
            );
            assert!(s.contains("mihomo-party"), "party sock unexpected: {s}");
        }
    }

    #[test]
    fn read_controller_yaml_only_sets_existing_sock_candidates() {
        let missing = format!("/tmp/clash-assist-no-sock-{}.sock", std::process::id());
        let _ = std::fs::remove_file(&missing);
        let r = read_controller_yaml(&[], 9090, 7890, &[missing], "test");
        assert!(r.sock_path.is_none());
        assert_ne!(r.sock_path.as_deref(), Some(platform::VERGE_SOCK_PATH));
    }

    #[test]
    fn merge_blank_ec_wins_over_stale_tcp_port() {
        let mut parsed = ParsedControllerYaml::default();
        merge_parsed(
            &mut parsed,
            parse_controller_yaml_rich(
                "external-controller: ''\nexternal-controller-unix: $TMPDIR/a.sock\nmixed-port: 7897\n",
            ),
        );
        merge_parsed(
            &mut parsed,
            parse_controller_yaml_rich(
                "external-controller: 127.0.0.1:9097\nexternal-controller-unix: /tmp/verge/verge-mihomo.sock\nmixed-port: 7897\n",
            ),
        );
        assert!(parsed.tcp_ec_blank);
        assert_eq!(parsed.port, None, "stale config.yaml must not revive TCP EC");
        assert_eq!(parsed.mixed_port, Some(7897));
        assert_eq!(
            parsed.unix_path.as_deref(),
            Some("$TMPDIR/a.sock"),
            "first unix path wins"
        );
    }

    #[test]
    fn blank_ec_parse_does_not_yield_default_port_via_rich() {
        let rich = parse_controller_yaml_rich("external-controller: ''\nmixed-port: 7897\n");
        assert_eq!(rich.port, None);
        assert!(rich.tcp_ec_blank);
        // Compat wrapper still fills default for non-Verge callers.
        let (port, _, mixed) = parse_controller_yaml(
            "external-controller: ''\nmixed-port: 7897\n",
            9097,
            1,
        );
        assert_eq!(port, 9097);
        assert_eq!(mixed, 7897);
    }

    #[test]
    fn live_verge_discover_resolves_service_sock_when_present() {
        let expected = platform::verge_service_sock_for_uid(platform::current_uid());
        #[allow(clippy::disallowed_methods, reason = "TODO(M1): 测试代码存量路径操作，M1 迁移（S-R1）")]
        let present = std::path::Path::new(&expected).exists();
        if !present {
            eprintln!("skip live: service sock absent ({expected})");
            return;
        }
        let d = discover_controller();
        eprintln!(
            "live discover: source={} sock={:?} port={}",
            d.source, d.sock_path, d.port
        );
        assert_eq!(
            d.sock_path.as_deref(),
            Some(expected.as_str()),
            "expected service sock, got {:?}",
            d.sock_path
        );
        assert!(
            d.source.contains("unix") || d.source.contains("verge-unix"),
            "source should reflect unix: {}",
            d.source
        );
    }

    #[test]
    fn parse_flclash_json_extracts_open_and_closed_ec() {
        // 1) shared_preferences / plist style where flutter.config is a stringified JSON
        let plist_json = r#"{
            "flutter.config": "{\"patchClashConfig\":{\"mixed-port\":7892,\"external-controller\":\"127.0.0.1:9090\",\"secret\":\"fl-secret-32chars\"}}"
        }"#;
        let parsed = parse_flclash_json_str(plist_json).expect("should parse");
        assert_eq!(parsed.port, Some(9090));
        assert!(!parsed.tcp_ec_blank);
        assert_eq!(parsed.secret, "fl-secret-32chars");
        assert_eq!(parsed.mixed_port, Some(7892));

        // 2) EC closed (default in FlClash)
        let closed_json = r#"{
            "flutter.config": "{\"patchClashConfig\":{\"mixed-port\":7890,\"external-controller\":\"\",\"secret\":\"\"}}"
        }"#;
        let closed = parse_flclash_json_str(closed_json).expect("should parse closed");
        assert_eq!(closed.port, None);
        assert!(closed.tcp_ec_blank);
        assert_eq!(closed.mixed_port, Some(7890));
    }

    #[test]
    fn discover_flclash_merges_prefs_and_yaml_and_flags_ec_disabled() {
        let tmp = PathBuf::from(format!("/tmp/clash-assist-flclash-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).expect("mkdir");

        let pref_file = tmp.join("shared_preferences.json");
        let yaml_file = tmp.join("config.yaml");

        // Case A: EC disabled in prefs
        std::fs::write(
            &pref_file,
            r#"{"flutter.config": "{\"patchClashConfig\":{\"mixed-port\":7895,\"external-controller\":\"\",\"secret\":\"\"}}"}"#,
        ).expect("write pref");

        let res_disabled = discover_flclash_with_paths(
            &[pref_file.display().to_string()],
            &[yaml_file.display().to_string()],
            9090,
            7890,
        );
        assert_eq!(res_disabled.port, 9090);
        assert_eq!(res_disabled.mixed_port, 7895);
        assert!(
            res_disabled.source.starts_with("flclash-ec-disabled:"),
            "unexpected source: {}",
            res_disabled.source
        );

        // Case A2: macOS binary plist via plutil
        #[cfg(target_os = "macos")]
        {
            let plist_file = tmp.join("com.follow.clash.plist");
            #[allow(clippy::disallowed_methods, reason = "仅 macOS（Unix）测试，不在 S-R1 的 Windows 风险面")]
            std::fs::write(&plist_file, std::fs::read(&pref_file).unwrap()).expect("copy json");
            let status = std::process::Command::new("/usr/bin/plutil")
                .args(["-convert", "binary1"])
                .arg(&plist_file)
                .status()
                .expect("plutil binary1");
            assert!(status.success());
            let res_plist = discover_flclash_with_paths(
                &[plist_file.display().to_string()],
                &[],
                9090,
                7890,
            );
            assert_eq!(res_plist.mixed_port, 7895);
            assert!(res_plist.source.starts_with("flclash-ec-disabled:"));
        }

        // Case B: EC enabled in prefs with 32-char secret
        std::fs::write(
            &pref_file,
            r#"{"flutter.config": "{\"patchClashConfig\":{\"mixed-port\":7896,\"external-controller\":\"127.0.0.1:9090\",\"secret\":\"my-flclash-secret\"}}"}"#,
        ).expect("write pref 2");
        let res_enabled = discover_flclash_with_paths(
            &[pref_file.display().to_string()],
            &[yaml_file.display().to_string()],
            9090,
            7890,
        );
        assert_eq!(res_enabled.port, 9090);
        assert_eq!(res_enabled.secret, "my-flclash-secret");
        assert_eq!(res_enabled.mixed_port, 7896);
        assert!(
            res_enabled.source.starts_with("flclash:"),
            "unexpected source: {}",
            res_enabled.source
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }
}

/// P-1：Windows 上发现结果不得携带 secret（§2.1 规则 4 / C-1 / C-4）。
#[cfg(all(test, windows))]
mod windows_secret_tests {
    use super::*;

    #[test]
    fn withhold_secret_clears_secret_and_keeps_rest() {
        let d = DiscoverResult {
            host: "127.0.0.1".into(),
            port: 9097,
            secret: "s3cret".into(),
            mixed_port: 7897,
            source: "verge-config:x".into(),
            sock_path: None,
        };
        let out = withhold_secret_on_windows(d);
        assert!(out.secret.is_empty());
        assert_eq!((out.port, out.mixed_port, out.source.as_str()), (9097, 7897, "verge-config:x"));
    }

    #[test]
    fn discover_entrypoints_never_return_secret_on_windows() {
        for id in ["verge", "flclash", "clashx_meta", "mihomo_party", "nope"] {
            assert!(discover_for_client(id).secret.is_empty(), "{id}");
        }
        assert!(discover_controller().secret.is_empty());
    }

    #[test]
    fn read_controller_yaml_never_probes_sock_candidates_on_windows() {
        let dir = std::env::temp_dir().join(format!("ca-win-sock-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let existing = dir.join("exists.sock");
        std::fs::write(&existing, b"").unwrap();
        let r = read_controller_yaml(
            &[],
            9090,
            7890,
            &[existing.to_string_lossy().into_owned()],
            "t",
        );
        assert_eq!(r.sock_path, None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
