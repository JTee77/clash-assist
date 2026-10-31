//! Tauri 命令边界上的出站约束（0.2.9 安全修复）。
//!
//! 前端一旦被注入脚本，`mihomo_http` / `egress_proxy_fetch` 等命令就是一个
//! 「替页面发任意 HTTP 请求」的跳板（SSRF，服务端请求伪造）。这里把它们收窄到
//! 应用实际用到的范围：
//! - 控制器请求：只允许回环地址（127.0.0.0/8、::1、localhost），路径只允许 Mihomo API 前缀；
//! - 出口探针：只允许固定的 https 探针主机；跳转只跟随 https 且不指向本机 / 内网。
//!
//! 约束放在命令层（lib.rs 调用）而不是传输函数里：传输层单测要连本地夹具服务器。

use std::net::IpAddr;

/// Mihomo REST API 中前端实际使用的一级路径（见 src/lib/mihomo/client.ts）。
const CONTROLLER_PATH_PREFIXES: &[&str] = &[
    "version",
    "configs",
    "proxies",
    "providers",
    "group",
    "connections",
    "rules",
];

/// `egress_proxy_fetch` 允许的探针主机（与前端 src/lib/egress/diagnostics/* 的 URL 一一对应）。
/// 新增探针时必须同步加到这里，否则 Rust 侧会拒绝。
pub const EGRESS_FETCH_HOSTS: &[&str] = &[
    // AI 可用性
    "chatgpt.com",
    "api.openai.com",
    "gemini.google.com",
    // 出口 IP / 地理信息
    "www.cloudflare.com",
    "cp.cloudflare.com",
    "get.geojs.io",
    "ipwho.is",
    "api.ip.sb",
    "api.ipify.org",
    "api64.ipify.org",
    "ipv6.icanhazip.com",
    // 连通性 / 延迟 / 分流
    "www.gstatic.com",
    "www.google.com",
    "play.google.com",
    "www.baidu.com",
    "www.qq.com",
    // 流媒体 / 商店
    "www.youtube.com",
    "www.disneyplus.com",
    "apps.apple.com",
    "www.netflix.com",
    "www.tiktok.com",
    "www.spotify.com",
    "www.primevideo.com",
];

/// `egress_proxy_timed_transfer`（带宽测速）允许的主机。
pub const EGRESS_TIMED_HOSTS: &[&str] = &["speed.cloudflare.com"];

/// 控制器主机必须是回环地址。
pub fn check_controller_host(host: &str) -> Result<(), String> {
    let h = host.trim();
    let bare = h
        .strip_prefix('[')
        .and_then(|x| x.strip_suffix(']'))
        .unwrap_or(h);
    let ok = bare.eq_ignore_ascii_case("localhost")
        || bare
            .parse::<IpAddr>()
            .map(|ip| ip.is_loopback())
            .unwrap_or(false);
    if ok {
        Ok(())
    } else {
        Err(format!("控制器地址只允许本机回环地址（收到 {h}）"))
    }
}

/// `.` / `..` 路径段（含 `%2e` 编码形式）。节点名里的 `HK..01` 这类不算。
fn is_dot_segment(seg: &str) -> bool {
    let s = seg.to_ascii_lowercase().replace("%2e", ".");
    s == "." || s == ".."
}

/// 控制器路径：必须以 `/` 开头、只含可见 ASCII（禁止 CR/LF/空格，防请求行注入），
/// 一级路径在 Mihomo API 白名单内，且不含 `.` / `..` 路径段。
pub fn check_controller_path(path: &str) -> Result<(), String> {
    let err = || format!("不允许的控制器路径：{path}");
    if !path.starts_with('/') {
        return Err(err());
    }
    let path_part = path.split(['?', '#']).next().unwrap_or("");
    if path_part.split('/').any(is_dot_segment) {
        return Err(err());
    }
    if !path.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(err());
    }
    let first = path[1..].split(['/', '?', '#']).next().unwrap_or("");
    if CONTROLLER_PATH_PREFIXES.contains(&first) {
        Ok(())
    } else {
        Err(err())
    }
}

/// 控制器 HTTP 方法：只允许前端实际用到的几种（Unix socket 路径是手写请求行，必须防注入）。
pub fn check_controller_method(method: &str) -> Result<(), String> {
    match method.to_ascii_uppercase().as_str() {
        "GET" | "PUT" | "POST" | "DELETE" | "PATCH" => Ok(()),
        _ => Err(format!("unsupported method: {method}")),
    }
}

/// Unix socket 路径：绝对路径，不含 `.` / `..` 段、NUL 与控制字符。
///
/// Windows 上前端传来的 `sockPath` 是命名管道名，转交 [`check_pipe_name`]（§2.2 L0）。
#[cfg(windows)]
pub fn check_sock_path(path: &str) -> Result<(), String> {
    check_pipe_name(path).map(|_| ())
}

/// Unix socket 路径：绝对路径，不含 `.` / `..` 段、NUL 与控制字符。
#[cfg(not(windows))]
pub fn check_sock_path(path: &str) -> Result<(), String> {
    if !path.starts_with('/')
        || path.split('/').any(|seg| seg == "." || seg == "..")
        || path.chars().any(|c| c.is_control())
    {
        return Err(format!("不允许的套接字路径：{path}"));
    }
    Ok(())
}

fn parse_https_no_extras(raw: &str) -> Option<url::Url> {
    let u = url::Url::parse(raw.trim()).ok()?;
    if u.scheme() != "https"
        || !u.username().is_empty()
        || u.password().is_some()
        || u.port().is_some()
    {
        return None;
    }
    Some(u)
}

fn check_egress_url_in(raw: &str, hosts: &[&str]) -> Result<(), String> {
    let u = parse_https_no_extras(raw).ok_or_else(|| format!("探针地址必须是 https：{raw}"))?;
    let host = u.host_str().unwrap_or("").to_ascii_lowercase();
    if hosts.iter().any(|h| *h == host) {
        Ok(())
    } else {
        Err(format!("探针主机不在允许列表中：{host}"))
    }
}

/// `egress_proxy_fetch` 的 URL 约束。
pub fn check_egress_fetch_url(raw: &str) -> Result<(), String> {
    check_egress_url_in(raw, EGRESS_FETCH_HOSTS)
}

/// `egress_proxy_timed_transfer` 的 URL 约束。
pub fn check_egress_timed_url(raw: &str) -> Result<(), String> {
    check_egress_url_in(raw, EGRESS_TIMED_HOSTS)
}

/// 探针跳转目标是否可以跟随：只跟随 https、默认端口、主机不是 IP 字面量 / localhost / 内网后缀。
/// 不满足时停止跟随（把 3xx 原样交回前端，探针按「有响应」处理），而不是报错。
pub fn redirect_target_ok(u: &url::Url) -> bool {
    if u.scheme() != "https" || u.port().is_some() || !u.username().is_empty() {
        return false;
    }
    match u.host() {
        Some(url::Host::Domain(d)) => {
            let d = d.trim_end_matches('.').to_ascii_lowercase();
            !(d == "localhost"
                || d.ends_with(".localhost")
                || d.ends_with(".local")
                || d.ends_with(".lan")
                || d.ends_with(".internal")
                || d.ends_with(".home.arpa")
                || !d.contains('.'))
        }
        // IP 字面量一律不跟随（探针只访问域名）
        _ => false,
    }
}

/// 探针用的跳转策略：最多 `max` 次，目标不合规时停止跟随。
pub fn probe_redirect_policy(max: usize) -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(move |attempt| {
        if attempt.previous().len() >= max {
            attempt.error("too many redirects")
        } else if redirect_target_ok(attempt.url()) {
            attempt.follow()
        } else {
            attempt.stop()
        }
    })
}

// ---- 命名管道名（windows-plan r4 §2.2 L0，S-R1） ----------------------------------------------

#[cfg(windows)]
pub use pipe_name::check_pipe_name;
#[cfg(any(windows, test))]
pub use pipe_name::{check_pipe_name_with_key, owner_key_from_sid, PipeName};

#[cfg(any(windows, test))]
mod pipe_name {
    use crate::mihomo::trust::PipeKind;

    /// 经过 [`check_pipe_name`] 校验的命名管道名（S-R1）。
    ///
    /// 字段私有，只能由 `check_pipe_name` 构造；**故意不实现** `AsRef<Path>`、`AsRef<OsStr>`、
    /// `AsRef<str>`、`Deref<Target = str>`、`Into<PathBuf>`、`Display`，也不提供公开的 `&str`
    /// 访问器：管道名不能被交给任何按路径打开的 API（V3：那些 API 以默认 QoS 打开管道，
    /// 服务端可以模拟本进程）。唯一的消费者是 `mihomo::pipe::open_verified`。
    ///
    /// ```compile_fail
    /// // 不能当作路径使用
    /// fn takes_path<P: AsRef<std::path::Path>>(_: P) {}
    /// fn f(n: clash_assist_lib::s_r1::PipeName) { takes_path(n) }
    /// ```
    ///
    /// ```compile_fail
    /// fn takes_os<P: AsRef<std::ffi::OsStr>>(_: P) {}
    /// fn f(n: clash_assist_lib::s_r1::PipeName) { takes_os(n) }
    /// ```
    ///
    /// ```compile_fail
    /// fn f(n: &clash_assist_lib::s_r1::PipeName) -> &str { &*n }
    /// ```
    ///
    /// ```compile_fail
    /// fn f(n: clash_assist_lib::s_r1::PipeName) -> std::path::PathBuf { n.into() }
    /// ```
    ///
    /// ```compile_fail
    /// fn f(n: &clash_assist_lib::s_r1::PipeName) -> String { n.to_string() }
    /// ```
    ///
    /// ```compile_fail
    /// // 不能在模块外构造
    /// let _ = clash_assist_lib::s_r1::PipeName(String::from(r"\\.\pipe\x"));
    /// ```
    ///
    /// 对照组（保证上面的 compile_fail 是因为缺少这些 trait，而不是路径写错）：
    ///
    /// ```no_run
    /// fn f(n: &clash_assist_lib::s_r1::PipeName) -> &clash_assist_lib::s_r1::PipeName { n }
    /// ```
    #[derive(Clone, PartialEq, Eq)]
    pub struct PipeName(String);

    impl PipeName {
        /// 仅供 `mihomo::pipe`（打开）与日志使用；不要把它交给任何文件系统 API。
        pub(in crate::mihomo) fn raw(&self) -> &str {
            &self.0
        }

        /// 测试用：构造任意（以 `\\.\pipe\` 开头的）管道名，便于并行测试使用唯一名字。
        #[cfg(all(test, windows))]
        pub(in crate::mihomo) fn for_test(name: String) -> Self {
            assert!(name.starts_with(PREFIX));
            PipeName(name)
        }
    }

    impl std::fmt::Debug for PipeName {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "PipeName({:?})", self.0)
        }
    }

    const PREFIX: &str = r"\\.\pipe\";
    const BASE: &str = "verge-mihomo";

    /// owner key = hex(sha256(当前用户 SID 字符串))，小写（命名契约，V5 已用真实 SID 核对）。
    pub fn owner_key_from_sid(sid: &str) -> String {
        use sha2::{Digest, Sha256};
        let d = Sha256::digest(sid.as_bytes());
        d.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// 生产入口：key 取当前用户的 owner key。
    #[cfg(windows)]
    pub fn check_pipe_name(name: &str) -> Result<(PipeName, PipeKind), String> {
        let key =
            crate::win::identity::owner_key().map_err(|e| format!("读不到当前用户 SID：{e}"))?;
        check_pipe_name_with_key(name, &key)
    }

    /// §2.2 L0：
    /// - 必须以 `\\.\pipe\` 开头；拒绝 `\\?\`、UNC、`/`、`..`、控制字符；
    /// - 名字部分匹配 `^verge-mihomo(-sidecar-(release|dev)|-production|-development)?(-[0-9a-f]{64})?$`；
    /// - 带 key 时 key 必须等于 `owner_key`。
    pub fn check_pipe_name_with_key(
        name: &str,
        owner_key: &str,
    ) -> Result<(PipeName, PipeKind), String> {
        let bad = |why: &str| Err(format!("不允许的命名管道名（{why}）：{name:?}"));
        if name.chars().any(|c| c.is_control()) {
            return bad("含控制字符");
        }
        if name.contains('/') || name.contains("..") {
            return bad("含 / 或 ..");
        }
        let Some(rest) = name.strip_prefix(PREFIX) else {
            return bad(r"必须以 \\.\pipe\ 开头");
        };
        if rest.contains('\\') {
            return bad("名字部分含反斜杠");
        }
        let Some(mut tail) = rest.strip_prefix(BASE) else {
            return bad("不是 verge-mihomo 管道");
        };
        let kind = if let Some(t) = tail
            .strip_prefix("-sidecar-release")
            .or_else(|| tail.strip_prefix("-sidecar-dev"))
        {
            tail = t;
            PipeKind::Sidecar
        } else if let Some(t) = tail
            .strip_prefix("-production")
            .or_else(|| tail.strip_prefix("-development"))
        {
            tail = t;
            PipeKind::Service
        } else {
            PipeKind::Legacy
        };
        match tail {
            "" => {}
            t => {
                let Some(key) = t.strip_prefix('-') else {
                    return bad("后缀格式错误");
                };
                if key.len() != 64 || !key.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
                    return bad("后缀格式错误");
                }
                if kind == PipeKind::Legacy {
                    // `verge-mihomo-<key>` 不在命名契约里。
                    return bad("不在命名契约中");
                }
                if key != owner_key {
                    return bad("owner key 与当前用户不符");
                }
            }
        }
        Ok((PipeName(name.to_string()), kind))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn controller_host_loopback_only() {
        for ok in [
            "127.0.0.1",
            "127.1.2.3",
            "localhost",
            "LOCALHOST",
            "::1",
            "[::1]",
            " 127.0.0.1 ",
        ] {
            assert!(check_controller_host(ok).is_ok(), "{ok}");
        }
        for bad in [
            "192.168.1.1",
            "10.0.0.1",
            "0.0.0.0",
            "169.254.169.254",
            "example.com",
            "localhost.evil.example",
            "127.0.0.1.nip.io",
            "",
            "::ffff:192.168.1.1",
        ] {
            assert!(check_controller_host(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn controller_path_allowlist() {
        for ok in [
            "/version",
            "/configs",
            "/proxies",
            "/proxies/%E8%8A%82%E7%82%B9/delay?timeout=5000&url=http%3A%2F%2Fwww.gstatic.com%2Fgenerate_204",
            "/providers/proxies",
            "/providers/proxies/sub/node/healthcheck?timeout=1",
            "/proxies/HK..01",
            "/connections",
            "/rules",
        ] {
            assert!(check_controller_path(ok).is_ok(), "{ok}");
        }
        for bad in [
            "version",
            "/",
            "/debug/pprof",
            "/restart",
            "/upgrade",
            "/proxies/../restart",
            "/proxies/%2e%2E/restart",
            "/proxies/./x",
            "/version HTTP/1.1\r\nHost: x",
            "/version\nX: y",
            "/proxies/a b",
            "/versionX",
        ] {
            assert!(check_controller_path(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn egress_fetch_allowlist() {
        assert!(check_egress_fetch_url("https://cp.cloudflare.com/cdn-cgi/trace").is_ok());
        assert!(check_egress_fetch_url("https://www.netflix.com/title/80018499").is_ok());
        assert!(check_egress_fetch_url("https://API.IPIFY.ORG?format=json").is_ok());
        for bad in [
            "http://cp.cloudflare.com/cdn-cgi/trace",
            "https://cp.cloudflare.com:8443/",
            "https://user@cp.cloudflare.com/",
            "https://evil.example/",
            "https://cp.cloudflare.com.evil.example/",
            "https://127.0.0.1/",
            "https://localhost/",
            "file:///etc/passwd",
            "https://speed.cloudflare.com/__down?bytes=1",
            "not a url",
        ] {
            assert!(check_egress_fetch_url(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn egress_timed_allowlist() {
        assert!(check_egress_timed_url("https://speed.cloudflare.com/__down?bytes=1000").is_ok());
        assert!(check_egress_timed_url("https://speed.cloudflare.com/__up").is_ok());
        assert!(check_egress_timed_url("https://www.google.com/").is_err());
        assert!(check_egress_timed_url("http://speed.cloudflare.com/__up").is_err());
    }

    #[test]
    fn redirect_targets() {
        let ok = |s: &str| redirect_target_ok(&url::Url::parse(s).unwrap());
        assert!(ok("https://www.netflix.com/hk/"));
        assert!(ok("https://consent.youtube.com/m?continue=x"));
        assert!(!ok("http://www.netflix.com/"));
        assert!(!ok("https://127.0.0.1/"));
        assert!(!ok("https://[::1]/"));
        assert!(!ok("https://192.168.1.1/"));
        assert!(!ok("https://localhost/"));
        assert!(!ok("https://router.local/"));
        assert!(!ok("https://nas.lan/"));
        assert!(!ok("https://intranet/"));
        assert!(!ok("https://www.netflix.com:8443/"));
    }

    #[test]
    fn controller_method_allowlist() {
        assert!(check_controller_method("get").is_ok());
        assert!(check_controller_method("DELETE").is_ok());
        assert!(check_controller_method("GET /x HTTP/1.1\r\n").is_err());
        assert!(check_controller_method("CONNECT").is_err());
    }

    #[cfg(not(windows))]
    #[test]
    fn sock_path_shape() {
        assert!(
            check_sock_path("/var/run/clash-verge-service/users/501/verge-mihomo.sock").is_ok()
        );
        assert!(check_sock_path("relative.sock").is_err());
        assert!(check_sock_path("/tmp/../var/run/docker.sock").is_err());
        assert!(check_sock_path("/tmp/a\0b").is_err());
    }

    /// 防漂移：前端 src/lib/egress 下出现的每个 https 探针主机都必须在 Rust 允许列表里，
    /// 否则新加的探针在生产包里会被拒绝（只在开发机上测出来就晚了）。
    #[test]
    fn frontend_probe_hosts_are_allowlisted() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/lib/egress");
        let re = regex::Regex::new(r"https://([A-Za-z0-9.-]+)").unwrap();
        let mut stack = vec![root];
        let mut seen = 0;
        while let Some(dir) = stack.pop() {
            for entry in crate::safe_fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if crate::safe_fs::is_dir(&path) {
                    stack.push(path);
                    continue;
                }
                let name = path.file_name().unwrap().to_string_lossy().into_owned();
                if !name.ends_with(".ts") || name.contains(".test.") {
                    continue;
                }
                let src = crate::safe_fs::read_to_string(&path).unwrap();
                for cap in re.captures_iter(&src) {
                    let host = cap[1].to_ascii_lowercase();
                    seen += 1;
                    assert!(
                        EGRESS_FETCH_HOSTS.contains(&host.as_str())
                            || EGRESS_TIMED_HOSTS.contains(&host.as_str()),
                        "{} uses https://{host} which is not in guard.rs allowlist",
                        path.display()
                    );
                }
            }
        }
        assert!(
            seen > 10,
            "expected to find probe URLs under src/lib/egress"
        );
    }

    // ---- 命名管道名 ----------------------------------------------------------------------

    const KEY: &str = "9a4913d151d61586ff28cde5a221651dde139248019801face68f1433157d564";

    #[test]
    fn owner_key_vector() {
        // V5 实测的 runner SID 与其 owner key（v5-spike-result-pit.md）。
        assert_eq!(
            owner_key_from_sid("S-1-5-21-3977851545-3833041388-3993362562-1003"),
            KEY
        );
    }

    #[test]
    fn pipe_name_positive() {
        use crate::mihomo::trust::PipeKind::*;
        for (n, kind) in [
            (
                format!(r"\\.\pipe\verge-mihomo-sidecar-release-{KEY}"),
                Sidecar,
            ),
            (format!(r"\\.\pipe\verge-mihomo-sidecar-dev-{KEY}"), Sidecar),
            (format!(r"\\.\pipe\verge-mihomo-production-{KEY}"), Service),
            (format!(r"\\.\pipe\verge-mihomo-development-{KEY}"), Service),
            (r"\\.\pipe\verge-mihomo".to_string(), Legacy),
            (r"\\.\pipe\verge-mihomo-production".to_string(), Service),
            (
                r"\\.\pipe\verge-mihomo-sidecar-release".to_string(),
                Sidecar,
            ),
        ] {
            let (p, k) = check_pipe_name_with_key(&n, KEY).unwrap_or_else(|e| panic!("{n}: {e}"));
            assert_eq!(k, kind, "{n}");
            assert_eq!(p.raw(), n);
        }
    }

    #[test]
    fn pipe_name_negative() {
        let other_key = "0".repeat(64);
        let upper = KEY.to_ascii_uppercase();
        let bad = [
            format!(r"\\.\pipe\verge-mihomo-sidecar-release-{other_key}"),
            format!(r"\\.\pipe\verge-mihomo-sidecar-release-{upper}"),
            format!(r"\\.\pipe\verge-mihomo-sidecar-release-{}", &KEY[..63]),
            format!(r"\\.\pipe\verge-mihomo-sidecar-release-{KEY}0"),
            format!(r"\\.\pipe\verge-mihomo-{KEY}"),
            format!(r"\\?\pipe\verge-mihomo-production-{KEY}"),
            format!(r"\\server\pipe\verge-mihomo-production-{KEY}"),
            format!(r"//./pipe/verge-mihomo-production-{KEY}"),
            format!(r"\\.\pipe\..\verge-mihomo-production-{KEY}"),
            format!(r"\\.\pipe\x\verge-mihomo-production-{KEY}"),
            format!("\\\\.\\pipe\\verge-mihomo-production-{KEY}\0"),
            "\\\\.\\pipe\\verge-mihomo\n".to_string(),
            r"\\.\PIPE\verge-mihomo".to_string(),
            r"\\.\pipe\verge-mihomo-sidecar".to_string(),
            r"\\.\pipe\verge-mihomo-sidecar-nightly".to_string(),
            r"\\.\pipe\verge-mihomo-production-".to_string(),
            r"\\.\pipe\verge-mihomo.sock".to_string(),
            r"\\.\pipe\mihomo".to_string(),
            r"\\.\pipe\".to_string(),
            r"C:\x\verge-mihomo".to_string(),
            "/tmp/verge/verge-mihomo.sock".to_string(),
            String::new(),
        ];
        for n in bad {
            assert!(check_pipe_name_with_key(&n, KEY).is_err(), "{n:?} 应被拒绝");
        }
    }

    /// S-R1：PipeName 不实现任何能把它交给路径 API 的 trait（类型层面的补充检查；
    /// compile_fail 文档测试见 `PipeName` 的文档）。
    #[test]
    fn pipe_name_has_no_path_like_impls() {
        use std::marker::PhantomData;
        trait No {
            const YES: bool = false;
        }
        macro_rules! probe {
            ($name:ident, $($bound:tt)+) => {
                struct $name<T>(PhantomData<T>);
                impl<T> No for $name<T> {}
                #[allow(dead_code)]
                impl<T: $($bound)+> $name<T> {
                    const YES: bool = true;
                }
            };
        }
        probe!(AsPath, AsRef<std::path::Path>);
        probe!(AsOs, AsRef<std::ffi::OsStr>);
        probe!(AsStr, AsRef<str>);
        probe!(Der, std::ops::Deref<Target = str>);
        probe!(IntoPb, Into<std::path::PathBuf>);
        probe!(IntoStr, Into<String>);
        probe!(Disp, std::fmt::Display);
        const { assert!(!AsPath::<PipeName>::YES) };
        const { assert!(!AsOs::<PipeName>::YES) };
        const { assert!(!AsStr::<PipeName>::YES) };
        const { assert!(!Der::<PipeName>::YES) };
        const { assert!(!IntoPb::<PipeName>::YES) };
        const { assert!(!IntoStr::<PipeName>::YES) };
        const { assert!(!Disp::<PipeName>::YES) };
        // 对照：String 确实会被探测到
        const { assert!(AsPath::<String>::YES) };
    }
}
