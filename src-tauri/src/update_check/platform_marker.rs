//! 「平台无变化不提示更新」标记（文档 46）。
//!
//! 发布说明（release body）里放一行 HTML 注释（页面上不显示）：
//!
//! ```text
//! <!-- clash-assist:platforms macos=0.3.1 windows=0.3.2 -->
//! ```
//!
//! 含义：每个平台**最近一次有改动的版本**。本平台的值不高于当前已安装版本时，说明从当前版本到
//! latest 本平台没有变化，不提示更新；等本平台下次真有改动（值变成新版本号）再提示，直接跳到最新版。
//! 记「最近改动版本」而不是「本版无变化」，是为了连续跳过多个版本时，停在更早版本的用户仍会收到提示。
//!
//! fail open：标记缺失、出现多次、格式不对、键未知或重复、版本号非法、值高于 latest、
//! 本平台的键缺失——一律视为「有变化」，照常提示更新。
//! 调用方只在 release 通过全部校验（含 immutable，见 `parse_release_asset`）时才采信本标记。

use semver::Version;

/// 标记关键字（发布说明模板与 scripts/check-release-notes.mjs 使用同一字符串）。
pub const MARKER_KEY: &str = "clash-assist:platforms";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    MacOs,
    Windows,
}

/// 解析出的标记：各平台最近一次有改动的版本。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PlatformMarker {
    pub macos: Option<Version>,
    pub windows: Option<Version>,
}

impl PlatformMarker {
    pub fn get(&self, p: Platform) -> Option<&Version> {
        match p {
            Platform::MacOs => self.macos.as_ref(),
            Platform::Windows => self.windows.as_ref(),
        }
    }
}

/// 从发布说明中解析标记；缺失或任何格式问题都返回 `None`。
pub fn parse(body: &str) -> Option<PlatformMarker> {
    // 只允许出现一次（多处标记无法判断以哪处为准）
    if body.matches(MARKER_KEY).count() != 1 {
        return None;
    }
    let line = body.lines().find(|l| l.contains(MARKER_KEY))?.trim();
    let inner = line.strip_prefix("<!--")?.strip_suffix("-->")?.trim();
    let rest = inner.strip_prefix(MARKER_KEY)?;
    // 关键字后必须是空白（拒绝 `clash-assist:platformsX`）
    if !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let mut m = PlatformMarker::default();
    let mut any = false;
    for tok in rest.split_whitespace() {
        let (k, v) = tok.split_once('=')?;
        let ver = Version::parse(v).ok()?; // 不接受 `v` 前缀
        let slot = match k {
            "macos" => &mut m.macos,
            "windows" => &mut m.windows,
            _ => return None,
        };
        if slot.replace(ver).is_some() {
            return None; // 重复键
        }
        any = true;
    }
    any.then_some(m)
}

/// 本平台从 `current` 到 `latest` 是否没有变化（true = 不提示更新）。
/// 任一版本号非法、标记缺本平台、值高于 latest 时返回 false（照常提示）。
pub fn unchanged_since(marker: &PlatformMarker, p: Platform, current: &str, latest: &str) -> bool {
    let (Some(cur), Some(lat)) = (super::parse_version(current), super::parse_version(latest))
    else {
        return false;
    };
    let Some(changed) = marker.get(p) else {
        return false;
    };
    use std::cmp::Ordering::*;
    changed.cmp_precedence(&lat) != Greater && changed.cmp_precedence(&cur) != Greater
}

/// 便捷入口：直接从发布说明判断。
pub fn body_says_unchanged(body: Option<&str>, p: Platform, current: &str, latest: &str) -> bool {
    body.and_then(parse)
        .is_some_and(|m| unchanged_since(&m, p, current, latest))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    const NOTES: &str = "## Clash伴侣 0.3.2\n\n<!-- clash-assist:platforms macos=0.3.1 windows=0.3.2 -->\n\n### Windows\n";

    #[test]
    fn parses_both_platforms() {
        let m = parse(NOTES).unwrap();
        assert_eq!(m.macos, Some(v("0.3.1")));
        assert_eq!(m.windows, Some(v("0.3.2")));
        // CRLF（网页编辑后的发布说明）、多余空白、键顺序不同
        let crlf = "a\r\n  <!--   clash-assist:platforms   windows=0.3.2  macos=0.3.1 -->\r\nb";
        assert_eq!(parse(crlf), Some(m));
    }

    #[test]
    fn single_platform_and_missing_key() {
        let m = parse("<!-- clash-assist:platforms windows=0.3.2 -->").unwrap();
        assert_eq!(m.macos, None);
        // 缺 macos 键 → Mac 照常提示
        assert!(!unchanged_since(&m, Platform::MacOs, "0.3.1", "0.3.2"));
        assert!(unchanged_since(&m, Platform::Windows, "0.3.2", "0.3.2"));
    }

    #[test]
    fn absent_or_malformed_is_none() {
        for bad in [
            "",
            "## Clash伴侣 0.3.2\n没有标记",
            "<!-- clash-assist:platforms -->",
            "<!-- clash-assist:platforms macos=0.3.1",
            "clash-assist:platforms macos=0.3.1",
            "<!-- clash-assist:platformsmacos=0.3.1 -->",
            "<!-- clash-assist:platforms macos=v0.3.1 -->",
            "<!-- clash-assist:platforms macos=0.3 -->",
            "<!-- clash-assist:platforms macos=0.3.1 windows -->",
            "<!-- clash-assist:platforms macos=0.3.1 linux=0.3.1 -->",
            "<!-- clash-assist:platforms macos=0.3.1 macos=0.3.0 -->",
            "<!-- clash-assist:platforms macos=0.3.1 --> 尾部文字",
            "<!-- clash-assist:platforms macos={{VERSION}} windows={{VERSION}} -->",
            // 出现两次
            "<!-- clash-assist:platforms macos=0.3.1 -->\n<!-- clash-assist:platforms macos=0.3.2 -->",
            "<!-- clash-assist:platforms macos=0.3.1 -->\n正文提到 clash-assist:platforms",
        ] {
            assert_eq!(parse(bad), None, "应视为无标记：{bad:?}");
        }
    }

    #[test]
    fn unchanged_rules() {
        let m = parse(NOTES).unwrap();
        // Mac 最近改动在 0.3.1：0.3.1 用户不提示；0.3.0 用户错过了 0.3.1 → 提示
        assert!(unchanged_since(&m, Platform::MacOs, "0.3.1", "0.3.2"));
        assert!(!unchanged_since(&m, Platform::MacOs, "0.3.0", "0.3.2"));
        // Windows 本版有改动 → 0.3.1 用户提示
        assert!(!unchanged_since(&m, Platform::Windows, "0.3.1", "0.3.2"));
        // 值高于 latest（填错）→ 提示
        let wrong = parse("<!-- clash-assist:platforms macos=0.3.3 -->").unwrap();
        assert!(!unchanged_since(&wrong, Platform::MacOs, "0.3.3", "0.3.2"));
        // 当前版本号非法 → 提示
        assert!(!unchanged_since(&m, Platform::MacOs, "dev", "0.3.2"));
        // 预发布：0.3.1-beta.1 < 0.3.1 → 提示
        assert!(!unchanged_since(
            &m,
            Platform::MacOs,
            "0.3.1-beta.1",
            "0.3.2"
        ));
    }

    #[test]
    fn body_helper_fails_open() {
        assert!(!body_says_unchanged(
            None,
            Platform::MacOs,
            "0.3.1",
            "0.3.2"
        ));
        assert!(!body_says_unchanged(
            Some("坏标记 clash-assist:platforms"),
            Platform::MacOs,
            "0.3.1",
            "0.3.2"
        ));
        assert!(body_says_unchanged(
            Some(NOTES),
            Platform::MacOs,
            "0.3.1",
            "0.3.2"
        ));
    }
}
