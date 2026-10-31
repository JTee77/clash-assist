//! 编码往返、前端解析器镜像、语法检查与基于生成标签的性质测试（不引入新依赖：自带 xorshift）。

use super::*;
use crate::mihomo::trust::{ListenerCheck, TcpFallback, TcpReject};

// ---- 解码（与前端 decodeURIComponent + RAW_RESERVED 检查等价的子集） -----------------------

/// 前端 `decodeValue`：出现未编码的 `= , )` → 失败；`%` 后必须是两位十六进制且解出合法 UTF-8。
fn decode_value(raw: &str) -> Option<String> {
    if raw.contains(['=', ',', ')']) {
        return None;
    }
    pct_decode(raw)
}

fn pct_decode(raw: &str) -> Option<String> {
    let b = raw.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let h = raw.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(h, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// 前端 `decodeList`。
fn decode_list(raw: &str) -> Option<Vec<String>> {
    if raw.is_empty() {
        return None;
    }
    raw.split('|')
        .map(|p| decode_value(p).filter(|v| !v.trim().is_empty()))
        .collect()
}

/// 前端 `parseSource` 的镜像（只为测试服务）：base、TCP 括号项、各后缀键的解码值。
#[derive(Debug)]
struct Parsed {
    base: String,
    tcp_items: Option<Vec<String>>,
    suffixes: Vec<(String, Option<Vec<String>>)>,
}

fn parse_like_frontend(source: &str) -> Parsed {
    let mut segs = source.split(';');
    let base = segs.next().unwrap_or("").to_string();
    let tcp_items = base.strip_prefix("verge-tcp(").and_then(|rest| {
        let inner = &rest[..rest.find(')')?];
        Some(
            inner
                .split(',')
                .map(|p| p.trim().to_string())
                .filter(|p| !p.is_empty())
                .collect(),
        )
    });
    let suffixes = segs
        .map(|seg| match seg.find('=') {
            Some(eq) => (seg[..eq].trim().to_string(), decode_list(&seg[eq + 1..])),
            None => (seg.trim().to_string(), None),
        })
        .collect();
    Parsed {
        base,
        tcp_items,
        suffixes,
    }
}

// ---- 语法检查（模块文档里的 ABNF） ----------------------------------------------------------

fn is_kind_s(s: &str) -> bool {
    is_kind(s)
}

fn check_part(p: &str) -> Result<(), String> {
    if p.is_empty() {
        return Err("空项".into());
    }
    if p.bytes()
        .any(|b| b < 0x20 || b == 0x7F || b";=|,()".contains(&b))
    {
        return Err(format!("项含未编码的保留 / 控制字符：{p:?}"));
    }
    pct_decode(p).ok_or_else(|| format!("项无法解码：{p:?}"))?;
    Ok(())
}

fn check_flag(f: &str) -> Result<(), String> {
    const FLAGS: &[&str] = &[
        "listener-verified",
        "listener-unverified",
        "service-mode",
        "pipe-insecure",
    ];
    if FLAGS.contains(&f) {
        return Ok(());
    }
    match f.strip_prefix("rejected:") {
        Some(code) if is_name(code) => Ok(()),
        _ => Err(format!("未知括号项：{f:?}")),
    }
}

/// 检查整条标签是否符合语法；返回 base 的 body（未解码）。
fn check_grammar(label: &str) -> Result<(), String> {
    if label.bytes().any(|b| b < 0x20 || b == 0x7F) {
        return Err("含控制字符".into());
    }
    let mut segs = label.split(';');
    let base = segs.next().unwrap();
    // base
    if base != "defaults" {
        // 括号在冒号之前时（verge-tcp(rejected:…)），冒号要从右括号之后找。
        let first_colon = base.find(':').ok_or("base 缺少冒号")?;
        let colon = match base.find('(') {
            Some(open) if open < first_colon => {
                let close = base.find(')').ok_or("括号未闭合")?;
                if base.as_bytes().get(close + 1) != Some(&b':') {
                    return Err("右括号后不是冒号".into());
                }
                close + 1
            }
            _ => first_colon,
        };
        let head = &base[..colon];
        let body = &base[colon + 1..];
        let kind = match head.find('(') {
            Some(open) => {
                let inner = head[open + 1..]
                    .strip_suffix(')')
                    .ok_or("括号未闭合或不在冒号前")?;
                if inner.contains(['(', ')']) {
                    return Err("嵌套括号".into());
                }
                if &head[..open] != "verge-tcp" {
                    return Err("只有 verge-tcp 带括号".into());
                }
                for f in inner.split(',') {
                    check_flag(f)?;
                }
                &head[..open]
            }
            None => head,
        };
        if head.find('(').is_none() && head.contains(')') {
            return Err("多余右括号".into());
        }
        if !is_kind_s(kind) {
            return Err(format!("kind 不合法：{kind:?}"));
        }
        if body.contains(['(', ')']) {
            return Err("body 含括号".into());
        }
        // body：固定结构 `defaults`、`<a>|yaml:<files>`，其余按 , | 拆项。
        // 空 body 合法：如 M-3 Blocked 且没读到 yaml 时的 `verge-pipe-untrusted:`。
        if body != "defaults" && !body.is_empty() {
            let body = body.replacen("|yaml:", "|", 1);
            for p in body.split([',', '|']) {
                check_part(p)?;
            }
        }
    }
    // 后缀
    let mut last_rank: Option<SuffixKey> = None;
    for seg in segs {
        let (k, v) = seg.split_once('=').ok_or("后缀缺 =")?;
        let key = SuffixKey::ALL
            .into_iter()
            .find(|x| x.as_str() == k)
            .ok_or_else(|| format!("未知键 {k:?}"))?;
        if let Some(prev) = last_rank {
            if key <= prev {
                return Err(format!("后缀顺序错误或重复：{k}"));
            }
        }
        last_rank = Some(key);
        for item in v.split('|') {
            check_part(item)?;
            if pct_decode(item).is_some_and(|d| d.trim().is_empty()) {
                return Err(format!("后缀值含空白项：{seg:?}"));
            }
        }
    }
    Ok(())
}

// ---- 编码单测 -------------------------------------------------------------------------------

#[test]
fn encode_reserved_and_controls() {
    assert_eq!(encode("a;b=c|d,e)f(g%h"), "a%3Bb%3Dc%7Cd%2Ce%29f%28g%25h");
    assert_eq!(encode("x\ny\t\u{7f}\0"), "x%0Ay%09%7F%00");
    // 中文、空格、反斜杠、冒号原样保留
    assert_eq!(
        encode(r"C:\Users\张三\AppData\Roaming\io.github.clash-verge-rev\config.yaml"),
        r"C:\Users\张三\AppData\Roaming\io.github.clash-verge-rev\config.yaml"
    );
    assert_eq!(encode("Program Files"), "Program Files");
    assert_eq!(encode(""), "");
}

#[test]
fn roundtrip_paths_with_semicolons() {
    let cases = [
        r"C:\Users\a;b\AppData\Roaming\io.github.clash-verge-rev\config.yaml",
        r"C:\a;pipe-insecure-dacl=x\config.yaml",
        r"C:\Program Files (x86)\v\clash-verge.yaml",
        r"\\.\pipe\verge-mihomo;x=1|2,3)",
        "100%;%25;%",
        "张三;李四",
    ];
    for c in cases {
        let e = encode(c);
        assert!(!e.contains([';', '=', '|', ',', '(', ')']), "{e}");
        assert_eq!(decode_value(&e).as_deref(), Some(c), "{e}");
    }
    // 文件列表：逐项编码后 `,` 连接，按 `,` 拆开逐项解码即还原
    let files = [cases[0], cases[1], cases[2]];
    let label = SourceLabel::files("verge-config", &files).finish();
    check_grammar(&label).unwrap();
    let body = label.strip_prefix("verge-config:").unwrap();
    assert!(!body.contains(';'), "{label}");
    let back: Vec<String> = body.split(',').map(|p| pct_decode(p).unwrap()).collect();
    assert_eq!(back, files);
    // 前端 base 不会被路径里的 `;` 截断，也不会凭空多出后缀
    let p = parse_like_frontend(&label);
    assert_eq!(p.base, label);
    assert!(p.suffixes.is_empty());
}

#[test]
fn suffix_values_roundtrip_and_order() {
    let label = SourceLabel::new("verge-pipe", encode("p"))
        .suffix(SuffixKey::PipeInsecureDacl, &["a;b", "c=d|e"])
        .suffix(SuffixKey::PipeOpenToAll, &["p,q)"])
        .suffix(SuffixKey::Ipv6DualStack, &["9097"])
        .suffix(SuffixKey::PipeInsecureDacl, &["f"])
        .suffix::<&str>(SuffixKey::PipeOpenToAll, &[])
        .finish();
    assert_eq!(
        label,
        "verge-pipe:p;ipv6-dual-stack=9097;pipe-open-to-all=p%2Cq%29;pipe-insecure-dacl=a%3Bb|c%3Dd%7Ce|f"
    );
    check_grammar(&label).unwrap();
    let p = parse_like_frontend(&label);
    assert_eq!(p.base, "verge-pipe:p");
    assert_eq!(
        p.suffixes,
        vec![
            ("ipv6-dual-stack".into(), Some(vec!["9097".into()])),
            ("pipe-open-to-all".into(), Some(vec!["p,q)".into()])),
            (
                "pipe-insecure-dacl".into(),
                Some(vec!["a;b".into(), "c=d|e".into(), "f".into()])
            ),
        ]
    );
}

/// 空白项换成占位符，键不会丢（pipe-insecure-dacl 的 fail-safe 提示必须保留）。
#[test]
fn blank_suffix_items_become_placeholder() {
    let label = SourceLabel::new("verge-pipe", encode("p"))
        .suffix(SuffixKey::PipeInsecureDacl, &["  ", "a"])
        .finish();
    assert_eq!(label, "verge-pipe:p;pipe-insecure-dacl=%28blank%29|a");
    check_grammar(&label).unwrap();
    let p = parse_like_frontend(&label);
    assert_eq!(
        p.suffixes,
        vec![(
            "pipe-insecure-dacl".into(),
            Some(vec![BLANK_ITEM.into(), "a".into()])
        )]
    );
}

#[test]
fn keys_are_lowercase_constants_and_insecure_dacl_is_last() {
    for k in SuffixKey::ALL {
        assert!(is_name(k.as_str()), "{k:?}");
        if k != SuffixKey::PipeInsecureDacl {
            assert!(!k.as_str().contains("pipe-insecure"), "{k:?}");
            assert!(k < SuffixKey::PipeInsecureDacl);
        }
    }
    let mut sorted = SuffixKey::ALL;
    sorted.sort();
    assert_eq!(sorted, SuffixKey::ALL);
    assert_eq!(*SuffixKey::ALL.last().unwrap(), SuffixKey::PipeInsecureDacl);
}

#[test]
fn tcp_flags_cover_all_checks() {
    let ok = TcpFallback::Allowed { denied_seen: false };
    let vo = TcpFallback::VerifiedOnly("x".into());
    let un = |svc| ListenerCheck::Unverified {
        pid: Some(1),
        reasons: vec![],
        service_mode: svc,
    };
    assert_eq!(
        tcp_flags(&un(true), &ok),
        Some(vec![TcpFlag::ListenerUnverified, TcpFlag::ServiceMode])
    );
    assert_eq!(
        tcp_flags(&un(true), &vo),
        Some(vec![
            TcpFlag::ListenerUnverified,
            TcpFlag::ServiceMode,
            TcpFlag::PipeInsecure
        ])
    );
    let rej = ListenerCheck::Rejected(TcpReject::ListenerConflict(vec![1, 2]));
    assert_eq!(
        tcp_flags(&rej, &vo),
        Some(vec![TcpFlag::Rejected("listener-conflict")])
    );
    let nl = ListenerCheck::NotListening {
        ipv6_dual_stack_hint: true,
    };
    assert_eq!(tcp_flags(&nl, &ok), None);
    for r in [
        TcpReject::ListenerConflict(vec![]),
        TcpReject::TokenMismatch {
            pid: 1,
            sid: "S-1-5-21-X".into(),
        },
        TcpReject::ImageMismatch {
            pid: 1,
            image: "X.EXE".into(),
        },
    ] {
        assert!(is_name(r.code()), "{r:?}");
    }
    let label = SourceLabel::tcp(
        &[TcpFlag::ListenerUnverified, TcpFlag::ServiceMode],
        &["C:/v/c.yaml"],
    )
    .finish();
    assert_eq!(
        label,
        "verge-tcp(listener-unverified,service-mode):C:/v/c.yaml"
    );
    let p = parse_like_frontend(&label);
    assert_eq!(
        p.tcp_items,
        Some(vec!["listener-unverified".into(), "service-mode".into()])
    );
}

#[test]
fn grammar_checker_rejects_bad_labels() {
    for bad in [
        "verge-pipe:a;b",
        "verge-pipe:a;Pipe-Insecure-Dacl=x",
        "verge-pipe:a;pipe-insecure-dacl=x=y",
        "verge-pipe:a;pipe-insecure-dacl=",
        "verge-pipe:a;pipe-insecure-dacl=a||b",
        "verge-pipe:a;pipe-insecure-dacl=x;ipv6-dual-stack=1",
        "verge-tcp(listener-verified:C:/x",
        "verge-tcp(listener-verified,(x)):C:/x",
        "verge-tcp(LISTENER-VERIFIED):C:/x",
        "verge-tcp(listener-verified):C:/(x)",
        "verge-config:a%2",
        "verge-config:a%FF",
        "verge-config:a\nb",
        "Verge-config:a",
    ] {
        assert!(check_grammar(bad).is_err(), "{bad}");
    }
}

// ---- 性质测试：随机路径 / 管道名 / 标记组合生成的标签都合乎语法，且前端能逐项还原 -------------

struct XorShift(u64);

impl XorShift {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn coin(&mut self) -> bool {
        self.next() & 1 == 1
    }
    fn string(&mut self) -> String {
        const ALPHABET: &[char] = &[
            'a', 'Z', '0', ' ', '\\', '/', ':', '.', '-', '_', ';', '=', '|', ',', '(', ')', '%',
            '\n', '\t', '\0', '\u{7f}', '\u{1f}', '张', '三', 'é', '😀', '#', '?', '&', '+', '"',
            '\'',
        ];
        let len = 1 + self.below(24);
        (0..len)
            .map(|_| ALPHABET[self.below(ALPHABET.len())])
            .collect()
    }
    fn nonblank_strings(&mut self, max: usize) -> Vec<String> {
        (0..self.below(max + 1))
            .map(|_| loop {
                let s = self.string();
                if !s.trim().is_empty() {
                    break s;
                }
            })
            .collect()
    }
}

#[test]
fn generated_labels_follow_grammar_and_roundtrip() {
    use super::super::discovery::windows_source_label as wsl;

    let mut rng = XorShift(0x9E37_79B9_7F4A_7C15);
    for _ in 0..4000 {
        let selected = rng.coin().then(|| rng.string());
        let open_to_all: Vec<String> = match &selected {
            Some(p) if rng.coin() => vec![p.clone()],
            _ => vec![],
        };
        let insecure = rng.nonblank_strings(3);
        let files = rng.nonblank_strings(3);
        let fallback = match rng.below(3) {
            0 => TcpFallback::Allowed { denied_seen: false },
            1 => TcpFallback::VerifiedOnly("x".into()),
            _ => TcpFallback::Blocked("x".into()),
        };
        let check = match rng.below(6) {
            0 => ListenerCheck::Verified {
                pid: 1,
                image: "verge-mihomo.exe".into(),
            },
            1 => ListenerCheck::Unverified {
                pid: None,
                reasons: vec![],
                service_mode: rng.coin(),
            },
            2 => ListenerCheck::Rejected(TcpReject::ListenerConflict(vec![1, 2])),
            3 => ListenerCheck::Rejected(TcpReject::TokenMismatch {
                pid: 1,
                sid: "S-1-5-21-1".into(),
            }),
            4 => ListenerCheck::Rejected(TcpReject::ImageMismatch {
                pid: 1,
                image: "x.exe".into(),
            }),
            _ => ListenerCheck::NotListening {
                ipv6_dual_stack_hint: rng.coin(),
            },
        };
        let port = rng.next() as u16;
        let tcp = rng.coin().then_some((&check, port));
        let label = wsl(
            selected.as_deref(),
            &open_to_all,
            &insecure,
            &fallback,
            &files,
            tcp,
            rng.coin(),
        );

        check_grammar(&label).unwrap_or_else(|e| panic!("{e}: {label:?}"));
        let p = parse_like_frontend(&label);

        // base 不被截断：base 里的路径 / 管道名逐项还原
        if let Some(pipe) = &selected {
            assert_eq!(p.base, format!("verge-pipe:{}", encode(pipe)), "{label:?}");
            assert_eq!(
                pct_decode(p.base.strip_prefix("verge-pipe:").unwrap()).as_deref(),
                Some(pipe.as_str())
            );
        } else if p.base != "defaults" && !files.is_empty() {
            let body_at = match p.base.find("):") {
                Some(i) if p.tcp_items.is_some() => i + 2,
                _ => p.base.find(':').unwrap() + 1,
            };
            let body = &p.base[body_at..];
            let back: Vec<String> = body.split(',').map(|x| pct_decode(x).unwrap()).collect();
            assert_eq!(back, files, "{label:?}");
        }

        // pipe-insecure 只作为括号内整项出现，且当且仅当回落状态为 VerifiedOnly + TCP 标签
        let tcp_has_insecure = p
            .tcp_items
            .as_ref()
            .is_some_and(|v| v.iter().any(|i| i == "pipe-insecure"));
        if tcp_has_insecure {
            assert!(
                matches!(fallback, TcpFallback::VerifiedOnly(_)),
                "{label:?}"
            );
        }
        if let Some(items) = &p.tcp_items {
            if items.iter().any(|i| i == "service-mode") {
                assert_eq!(items[0], "listener-unverified");
            }
        }

        // 后缀：每个键的值都能被前端解码回原值；pipe-insecure-dacl 在最后
        for (k, v) in &p.suffixes {
            let v = v
                .as_ref()
                .unwrap_or_else(|| panic!("{k} 无法解析：{label:?}"));
            match k.as_str() {
                "pipe-insecure-dacl" => assert_eq!(v, &insecure),
                "pipe-open-to-all" => {
                    let want: Vec<String> = open_to_all
                        .iter()
                        .map(|x| {
                            if x.trim().is_empty() {
                                BLANK_ITEM.to_string()
                            } else {
                                x.clone()
                            }
                        })
                        .collect();
                    assert_eq!(v, &want)
                }
                "ipv6-dual-stack" => assert_eq!(v, &vec![port.to_string()]),
                other => panic!("未知键 {other}"),
            }
        }
        if !insecure.is_empty() {
            assert_eq!(p.suffixes.last().unwrap().0, "pipe-insecure-dacl");
        } else {
            assert!(p.suffixes.iter().all(|(k, _)| k != "pipe-insecure-dacl"));
        }
        if let Some(pos) = label.find(";pipe-insecure-dacl=") {
            for k in ["ipv6-dual-stack", "pipe-open-to-all"] {
                if let Some(kp) = label.find(&format!(";{k}=")) {
                    assert!(kp < pos, "{label:?}");
                }
            }
        }
    }
}

/// macOS / 其他客户端的标签也走同一套编码（随机路径）。
#[test]
fn generated_plain_labels_follow_grammar() {
    let mut rng = XorShift(42);
    for _ in 0..2000 {
        let files = {
            let mut v = rng.nonblank_strings(3);
            if v.is_empty() {
                v.push(rng.string());
            }
            v
        };
        let sock = rng.string();
        let enc_files = encode_list(&files, ',');
        let labels = [
            SourceLabel::files("verge-config", &files).finish(),
            SourceLabel::files("flclash-ec-disabled", &files).finish(),
            SourceLabel::new(
                "verge-config+unix",
                format!("{enc_files}|{}", encode(&sock)),
            )
            .finish(),
            SourceLabel::new("verge-unix", format!("{}|yaml:{enc_files}", encode(&sock))).finish(),
            SourceLabel::new("clashx_meta", encode(&files[0])).finish(),
            SourceLabel::new("unknown-client", encode(&sock)).finish(),
        ];
        for l in labels {
            check_grammar(&l).unwrap_or_else(|e| panic!("{e}: {l:?}"));
            assert!(!l.contains(';'), "{l:?}");
        }
    }
}
