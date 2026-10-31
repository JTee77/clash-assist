//! 发现来源标签（`DiscoverResult.source`）的结构化构造与值编码（PR52 Ellie must-fix 1 的后端侧）。
//!
//! 前端 `src/lib/mihomo/sourceDiagnostics.ts::parseSource` 按下面的语法结构化解析，本模块是
//! 唯一的生产者：发现代码不再手写 `format!` 拼标签。
//!
//! ```text
//! label   = base *( ";" key "=" value )
//! base    = "defaults"
//!         / kind [ "(" flag *( "," flag ) ")" ] ":" body
//! kind    = 1*( a-z / 0-9 / "-" / "+" / "_" )    ; 编译期常量，如 verge-pipe、verge-config+unix、clashx_meta
//! flag    = name [ ":" name ]                     ; 固定常量，见 [`TcpFlag`]
//! name    = 1*( a-z / 0-9 / "-" )
//! body    = [ part *( ( "," / "|" ) part ) ]      ; 可为空（如没读到 yaml 时的 verge-pipe-untrusted:）
//!         / "defaults"                            ; 其他客户端没找到配置时
//!                                                 ; macOS 另有固定结构 `<sock>|yaml:<files>`
//! key     = name                                  ; 固定常量，见 [`SuffixKey`]
//! value   = item *( "|" item )                    ; 每项单独编码、非空
//! part / item = 1*( 非保留字符 / pct )
//! pct     = "%" HEXDIG HEXDIG                     ; 大写十六进制
//! ```
//!
//! - 保留字符 `; = | , ( ) %` 和控制字符（U+0000–U+001F、U+007F）在 **base 的路径 / 管道名**
//!   与**后缀值**里一律按 UTF-8 字节百分号编码（[`encode`]）；其他字符（含中文、空格、`\`、`:`）
//!   原样保留，前端 `decodeURIComponent` 可逐项还原。
//! - 后缀顺序固定：[`SuffixKey`] 的声明顺序，`pipe-insecure-dacl` 永远在最后；新键不含
//!   `pipe-insecure` 字样（前端只认 `pipe-insecure-dacl` 键和括号内整项 `pipe-insecure`）。
//! - 括号只出现在 `verge-tcp(...)`，不会嵌套、不会缺右括号：路径里的 `(` `)` 也被编码。

/// 需要编码的 ASCII 字符（另加所有控制字符）。
const RESERVED: &[u8] = b";=|,()%";

fn needs_encoding(b: u8) -> bool {
    b < 0x20 || b == 0x7F || RESERVED.contains(&b)
}

/// 百分号编码一个路径 / 管道名 / 值。只编码保留字符与控制字符，其余 UTF-8 原样保留。
pub fn encode(raw: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        // 保留字符与控制字符都是单字节 ASCII。
        if ch.is_ascii() && needs_encoding(ch as u8) {
            let b = ch as u8;
            out.push('%');
            out.push(HEX[usize::from(b >> 4)] as char);
            out.push(HEX[usize::from(b & 0xF)] as char);
        } else {
            out.push(ch);
        }
    }
    out
}

/// 逐项编码后用 `sep`（`,` 或 `|`）连接。
pub fn encode_list<S: AsRef<str>>(items: &[S], sep: char) -> String {
    debug_assert!(sep == ',' || sep == '|');
    let mut out = String::new();
    for (i, it) in items.iter().enumerate() {
        if i > 0 {
            out.push(sep);
        }
        out.push_str(&encode(it.as_ref()));
    }
    out
}

/// 后缀值里空白项的占位符（见 [`SourceLabel::suffix`]）。
pub const BLANK_ITEM: &str = "(blank)";

/// `;key=value` 后缀的键。声明顺序即输出顺序：新键在前，`pipe-insecure-dacl` 永远最后。
// macOS 发现不产生后缀，非测试构建里这些只有 Windows 用到。
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SuffixKey {
    /// S-R12：IPv4 回环上没有监听者，但 IPv6 表里同端口有 `[::]` 监听。值为十进制端口。
    Ipv6DualStack,
    /// S-2：选中的管道通过了 F3′，但 DACL 给 BU / AU / WD 授予了写数据权限（只提示，不拒绝）。
    PipeOpenToAll,
    /// F3′：有候选管道因 DACL / 标签允许外人插入实例被拒（InsecureDacl，Ellie PR50 Q5）。
    PipeInsecureDacl,
}

#[cfg_attr(not(any(windows, test)), allow(dead_code))]
impl SuffixKey {
    #[cfg(test)]
    pub const ALL: [SuffixKey; 3] = [
        SuffixKey::Ipv6DualStack,
        SuffixKey::PipeOpenToAll,
        SuffixKey::PipeInsecureDacl,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            SuffixKey::Ipv6DualStack => "ipv6-dual-stack",
            SuffixKey::PipeOpenToAll => "pipe-open-to-all",
            SuffixKey::PipeInsecureDacl => "pipe-insecure-dacl",
        }
    }
}

/// `verge-tcp(...)` 括号内的项（固定常量）。
#[cfg(any(windows, test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TcpFlag {
    ListenerVerified,
    ListenerUnverified,
    /// C-4：监听进程是 SYSTEM（clash-verge-service），TCP 恒为未验证。
    ServiceMode,
    /// 有管道因 InsecureDacl 被拒，TCP 只许 listener-verified（`TcpFallback::VerifiedOnly`）。
    PipeInsecure,
    /// 监听者校验拒绝；值为 `TcpReject::code()`（固定常量）。
    Rejected(&'static str),
}

#[cfg(any(windows, test))]
impl TcpFlag {
    fn write_to(self, out: &mut String) {
        match self {
            TcpFlag::ListenerVerified => out.push_str("listener-verified"),
            TcpFlag::ListenerUnverified => out.push_str("listener-unverified"),
            TcpFlag::ServiceMode => out.push_str("service-mode"),
            TcpFlag::PipeInsecure => out.push_str("pipe-insecure"),
            TcpFlag::Rejected(code) => {
                debug_assert!(is_name(code), "reject code {code:?}");
                out.push_str("rejected:");
                out.push_str(code);
            }
        }
    }
}

/// `[a-z0-9-]+`。
#[cfg(any(windows, test))]
pub fn is_name(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// 一个来源标签：base + 有序后缀。
#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use]
pub struct SourceLabel {
    base: String,
    suffixes: Vec<(SuffixKey, String)>,
}

impl SourceLabel {
    /// 不带冒号的 base（目前只有 `defaults`）。
    pub fn bare(kind: &'static str) -> Self {
        debug_assert!(is_kind(kind), "kind {kind:?}");
        SourceLabel {
            base: kind.to_string(),
            suffixes: Vec::new(),
        }
    }

    /// `kind:<body>`。`body` 必须已经由 [`encode`] / [`encode_list`] 编码（或是固定字面量）。
    pub fn new(kind: &'static str, body: String) -> Self {
        debug_assert!(is_kind(kind), "kind {kind:?}");
        SourceLabel {
            base: format!("{kind}:{body}"),
            suffixes: Vec::new(),
        }
    }

    /// `kind:<files>`：文件路径逐个编码，`,` 连接。
    pub fn files<S: AsRef<str>>(kind: &'static str, files: &[S]) -> Self {
        Self::new(kind, encode_list(files, ','))
    }

    /// `verge-tcp(<flags>):<files>`。
    #[cfg(any(windows, test))]
    pub fn tcp<S: AsRef<str>>(flags: &[TcpFlag], files: &[S]) -> Self {
        debug_assert!(!flags.is_empty());
        let mut base = String::from("verge-tcp(");
        for (i, f) in flags.iter().enumerate() {
            if i > 0 {
                base.push(',');
            }
            f.write_to(&mut base);
        }
        base.push_str("):");
        base.push_str(&encode_list(files, ','));
        SourceLabel {
            base,
            suffixes: Vec::new(),
        }
    }

    #[cfg_attr(not(any(windows, test)), allow(dead_code))]
    /// 追加 `;key=<v1|v2…>`（每项编码）。`values` 为空时不追加。同一键重复追加时合并。
    ///
    /// 空白项（trim 后为空）无法原样表达——前端 `decodeList` 会把整个值判为解析失败——所以换成
    /// 占位符 [`BLANK_ITEM`]（编码后为 `%28blank%29`），而**不**丢弃：`pipe-insecure-dacl` 键必须
    /// 出现，前端才会给出 fail-safe 提示。实际的管道名都经过 `guard` 校验，不会是空白。
    pub fn suffix<S: AsRef<str>>(mut self, key: SuffixKey, values: &[S]) -> Self {
        if values.is_empty() {
            return self;
        }
        let kept: Vec<&str> = values
            .iter()
            .map(|v| {
                let v = v.as_ref();
                if v.trim().is_empty() {
                    BLANK_ITEM
                } else {
                    v
                }
            })
            .collect();
        let enc = encode_list(&kept, '|');
        match self.suffixes.iter_mut().find(|(k, _)| *k == key) {
            Some((_, v)) => {
                v.push('|');
                v.push_str(&enc);
            }
            None => self.suffixes.push((key, enc)),
        }
        self
    }

    /// 输出：后缀按 [`SuffixKey`] 声明顺序排列（`pipe-insecure-dacl` 最后）。
    pub fn finish(mut self) -> String {
        self.suffixes.sort_by_key(|(k, _)| *k);
        let mut out = self.base;
        for (k, v) in self.suffixes {
            out.push(';');
            out.push_str(k.as_str());
            out.push('=');
            out.push_str(&v);
        }
        out
    }
}

fn is_kind(s: &str) -> bool {
    !s.is_empty()
        && s.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'-' | b'+' | b'_')
        })
}

/// Windows TCP 监听者判定 → `verge-tcp(...)` 括号内的项。`None`：不是 TCP 标签（无监听者）。
///
/// - Verified → `listener-verified`
/// - Unverified → `listener-unverified`，服务模式（C-4）再加 `service-mode`
/// - 回落状态为 `VerifiedOnly`（有 InsecureDacl 管道）时末尾加 `pipe-insecure`
/// - Rejected → `rejected:<code>`（不加 pipe-insecure，与 M1 行为一致）
#[cfg(any(windows, test))]
pub fn tcp_flags(
    check: &super::trust::ListenerCheck,
    fallback: &super::trust::TcpFallback,
) -> Option<Vec<TcpFlag>> {
    use super::trust::{ListenerCheck, TcpFallback};
    let mut flags = match check {
        ListenerCheck::Verified { .. } => vec![TcpFlag::ListenerVerified],
        ListenerCheck::Unverified { service_mode, .. } => {
            let mut v = vec![TcpFlag::ListenerUnverified];
            if *service_mode {
                v.push(TcpFlag::ServiceMode);
            }
            v
        }
        ListenerCheck::Rejected(r) => return Some(vec![TcpFlag::Rejected(r.code())]),
        ListenerCheck::NotListening { .. } => return None,
    };
    if matches!(fallback, TcpFallback::VerifiedOnly(_)) {
        flags.push(TcpFlag::PipeInsecure);
    }
    Some(flags)
}

#[cfg(test)]
mod tests;
