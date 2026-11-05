//! 「更新待完成」标记与启动对账（windows-plan r4 §2.4「启动、退出与失败恢复」，M-5）。
//!
//! 启动安装程序前写 `%LOCALAPPDATA%\ClashAssist\update-pending.json`：
//! `{from, to, sha256, installer_path, started_at}`。下次启动时对账：
//! - 当前版本等于 `to`：更新成功 → 清理标记与更新目录，提示「已更新」；
//! - 当前版本仍等于 `from`：上次更新未完成 → 清理旧目录（重试时重新下载并校验，不复用旧文件），
//!   提示「上次更新未完成」，提供「重试」与「前往发布页」；
//! - 标记超过 24 小时、无法解析、版本对不上：直接清理，不提示。
//!
//! 标记只在本平台的应用数据目录下读写；清理时只删除 `updates\<32 位十六进制>\` 形状的目录，
//! 不信任标记里写的任意路径。判定是纯函数（[`decide`]），各平台单测；IO 部分注入根目录。

// Mac 暂不写 pending 标记（plan：Windows 先用，Mac 可选）；非 Windows 构建中只用到 `Outcome::None`。
#![cfg_attr(not(windows), allow(dead_code))]

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use super::{same_version, VersionPair};

pub const MARKER_FILE: &str = "update-pending.json";
pub const UPDATES_DIR: &str = "updates";
/// 超过这个时长的标记直接清理（plan：24 小时）。
pub const MAX_AGE_SECS: u64 = 24 * 60 * 60;
/// 标记文件读取上限（正常不到 1 KB）。
const MAX_MARKER_BYTES: u64 = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingMarker {
    pub from: String,
    pub to: String,
    /// 安装包 SHA-256（小写十六进制），仅用于排查。
    pub sha256: String,
    pub installer_path: String,
    /// Unix 秒。
    pub started_at: u64,
}

/// 对账结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    None,
    Updated(VersionPair),
    Unfinished(VersionPair),
}

/// 纯判定：标记 + 当前版本 + 当前时间 → 结果。调用方无论结果如何都会清理标记与目录。
pub fn decide(marker: Option<&PendingMarker>, current: &str, now: u64) -> Outcome {
    let Some(m) = marker else {
        return Outcome::None;
    };
    // 时钟回拨超过 5 分钟（started_at 在未来）也按过期处理。
    if m.started_at > now.saturating_add(300) || now.saturating_sub(m.started_at) > MAX_AGE_SECS {
        return Outcome::None;
    }
    let pair = VersionPair {
        from: m.from.clone(),
        to: m.to.clone(),
    };
    if same_version(current, &m.to) {
        Outcome::Updated(pair)
    } else if same_version(current, &m.from) {
        Outcome::Unfinished(pair)
    } else {
        Outcome::None
    }
}

pub fn marker_path(root: &Path) -> PathBuf {
    root.join(MARKER_FILE)
}

pub fn updates_root(root: &Path) -> PathBuf {
    root.join(UPDATES_DIR)
}

/// `updates\` 下本次更新的工作目录名：32 位小写十六进制。
pub fn is_work_dir_name(name: &str) -> bool {
    name.len() == 32
        && name
            .chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
}

/// 标记里的安装包路径所在的工作目录——只有恰好是 `root\updates\<32 位十六进制>\<文件>` 时才返回。
pub fn work_dir_of(root: &Path, installer_path: &str) -> Option<PathBuf> {
    let p = Path::new(installer_path);
    let dir = p.parent()?;
    let name = dir.file_name()?.to_str()?;
    if !is_work_dir_name(name) || p.file_name().is_none() {
        return None;
    }
    let want = updates_root(root).join(name);
    (dir == want.as_path()).then_some(want)
}

pub fn write(root: &Path, m: &PendingMarker) -> Result<(), String> {
    use std::io::Write;
    let body = serde_json::to_vec(m).map_err(|e| format!("序列化更新标记失败：{e}"))?;
    let path = marker_path(root);
    let mut f =
        crate::safe_fs::create_truncate(&path).map_err(|e| format!("写入更新标记失败：{e}"))?;
    f.write_all(&body)
        .and_then(|_| f.sync_all())
        .map_err(|e| format!("写入更新标记失败：{e}"))
}

pub fn read(root: &Path) -> Option<PendingMarker> {
    use std::io::Read;
    let f = crate::safe_fs::open_read(&marker_path(root)).ok()?;
    let mut buf = Vec::new();
    f.take(MAX_MARKER_BYTES + 1).read_to_end(&mut buf).ok()?;
    if buf.len() as u64 > MAX_MARKER_BYTES {
        return None;
    }
    serde_json::from_slice(&buf).ok()
}

/// 删除标记文件；若标记里的安装包位于合法的工作目录，一并删除该目录。
pub fn clear(root: &Path, m: Option<&PendingMarker>) {
    if let Some(dir) = m.and_then(|m| work_dir_of(root, &m.installer_path)) {
        remove_work_dir(&dir);
    }
    let _ = std::fs::remove_file(marker_path(root));
}

/// 删除一个工作目录（`std::fs::remove_dir_all` 不跟随目录联接点 / 符号链接）。
pub fn remove_work_dir(dir: &Path) {
    let _ = std::fs::remove_dir_all(dir);
}

/// 启动时删除 `updates\` 下所有工作目录（只认 32 位十六进制名字；不跟随链接）。
/// 本进程此时还没有开始任何下载，所以这些都是以前遗留的：下载后没重启、安装程序已结束等。
/// 仍被占用的（例如安装程序还没退出）删不掉就留到下次启动。
pub fn sweep_work_dirs(root: &Path) -> usize {
    let Ok(rd) = crate::safe_fs::read_dir(&updates_root(root)) else {
        return 0;
    };
    let mut n = 0;
    for e in rd.flatten() {
        let name = e.file_name();
        let is_dir = e
            .file_type()
            .map(|t| t.is_dir() && !t.is_symlink())
            .unwrap_or(false);
        if is_dir && name.to_str().is_some_and(is_work_dir_name) {
            remove_work_dir(&e.path());
            n += 1;
        }
    }
    n
}

/// 读标记 → 判定 → 清理（任何结果都清理）。标记存在但无法解析时同样删除。
/// 日志行（e2e 接口约定第 6 点）：`[update] pending outcome=updated|unfinished|none from=… to=…`。
pub fn reconcile(root: &Path, current: &str, now: u64) -> Outcome {
    let marker = read(root);
    let exists = crate::safe_fs::exists(&marker_path(root));
    let outcome = decide(marker.as_ref(), current, now);
    if marker.is_some() || exists {
        clear(root, marker.as_ref());
        let (kind, pair) = match &outcome {
            Outcome::Updated(p) => ("updated", Some(p)),
            Outcome::Unfinished(p) => ("unfinished", Some(p)),
            Outcome::None => ("none", None),
        };
        crate::append_app_log(&match pair {
            Some(p) => format!(
                "[update] pending outcome={kind} from={} to={}",
                p.from, p.to
            ),
            None => format!("[update] pending outcome={kind}"),
        });
    }
    let swept = sweep_work_dirs(root);
    if swept > 0 {
        crate::append_app_log(&format!("[update] 启动时清理遗留更新目录 {swept} 个"));
    }
    outcome
}

pub fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 本次启动的对账结果：进程内只算一次（启动时 lib.rs 先触发，`check` 读取同一个结果）。
#[cfg(windows)]
pub fn startup_outcome() -> Outcome {
    use std::sync::OnceLock;
    static ONCE: OnceLock<Outcome> = OnceLock::new();
    ONCE.get_or_init(|| match crate::platform::app_data_dir() {
        Some(root) => reconcile(&root, env!("CARGO_PKG_VERSION"), now_unix()),
        None => Outcome::None,
    })
    .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marker(from: &str, to: &str, started_at: u64, installer_path: &str) -> PendingMarker {
        PendingMarker {
            from: from.into(),
            to: to.into(),
            sha256: "00".repeat(32),
            installer_path: installer_path.into(),
            started_at,
        }
    }

    fn pair(from: &str, to: &str) -> VersionPair {
        VersionPair {
            from: from.into(),
            to: to.into(),
        }
    }

    #[test]
    fn state_machine_success_unfinished_and_stale() {
        let now = 1_800_000_000;
        let m = marker("0.3.0", "0.3.1", now - 60, "x");
        assert_eq!(decide(None, "0.3.0", now), Outcome::None);
        // 成功：当前版本 == to（含 v 前缀写法）
        assert_eq!(
            decide(Some(&m), "0.3.1", now),
            Outcome::Updated(pair("0.3.0", "0.3.1"))
        );
        assert_eq!(
            decide(Some(&m), "v0.3.1", now),
            Outcome::Updated(pair("0.3.0", "0.3.1"))
        );
        // 未完成：当前版本仍 == from
        assert_eq!(
            decide(Some(&m), "0.3.0", now),
            Outcome::Unfinished(pair("0.3.0", "0.3.1"))
        );
        // 版本对不上（例如用户手动装了别的版本）
        assert_eq!(decide(Some(&m), "0.3.2", now), Outcome::None);
        // 恰好 24 小时仍有效；超过 24 小时直接清理
        let edge = marker("0.3.0", "0.3.1", now - MAX_AGE_SECS, "x");
        assert_eq!(
            decide(Some(&edge), "0.3.0", now),
            Outcome::Unfinished(pair("0.3.0", "0.3.1"))
        );
        let stale = marker("0.3.0", "0.3.1", now - MAX_AGE_SECS - 1, "x");
        assert_eq!(decide(Some(&stale), "0.3.0", now), Outcome::None);
        assert_eq!(decide(Some(&stale), "0.3.1", now), Outcome::None);
        // started_at 在未来（时钟回拨）超过 5 分钟：按过期处理
        let future = marker("0.3.0", "0.3.1", now + 301, "x");
        assert_eq!(decide(Some(&future), "0.3.0", now), Outcome::None);
        let near_future = marker("0.3.0", "0.3.1", now + 299, "x");
        assert_eq!(
            decide(Some(&near_future), "0.3.0", now),
            Outcome::Unfinished(pair("0.3.0", "0.3.1"))
        );
    }

    #[test]
    fn work_dir_must_be_a_random_child_of_updates() {
        let root = std::env::temp_dir().join("ca-pending-shape");
        let good = updates_root(&root).join("0123456789abcdef0123456789abcdef");
        let inst = good.join("setup-0123456789abcdef.exe");
        assert_eq!(
            work_dir_of(&root, &inst.to_string_lossy()),
            Some(good.clone())
        );
        for bad in [
            // 名字不是 32 位小写十六进制
            updates_root(&root)
                .join("0123456789ABCDEF0123456789ABCDEF")
                .join("s.exe"),
            updates_root(&root).join("short").join("s.exe"),
            // 不在 updates 下 / 多一层 / 直接是 updates 本身
            root.join("0123456789abcdef0123456789abcdef").join("s.exe"),
            good.join("sub").join("s.exe"),
            updates_root(&root).join("s.exe"),
            // 其他根目录
            std::env::temp_dir()
                .join("elsewhere")
                .join(UPDATES_DIR)
                .join("0123456789abcdef0123456789abcdef")
                .join("s.exe"),
        ] {
            assert_eq!(work_dir_of(&root, &bad.to_string_lossy()), None, "{bad:?}");
        }
        assert_eq!(work_dir_of(&root, ""), None);
    }

    #[test]
    fn startup_sweeps_only_work_dirs() {
        let root = std::env::temp_dir().join(format!("ca-sweep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let ups = updates_root(&root);
        let stale = ups.join("00112233445566778899aabbccddeeff");
        std::fs::create_dir_all(&stale).unwrap();
        crate::safe_fs::write_for_test(&stale.join("setup-0000000000000000.exe"), b"MZ").unwrap();
        // 名字不符合的目录与文件不碰
        let other = ups.join("keep-me");
        std::fs::create_dir_all(&other).unwrap();
        crate::safe_fs::write_for_test(&ups.join("00112233445566778899aabbccddeef0"), b"file")
            .unwrap();
        // 没有标记也清理
        assert_eq!(reconcile(&root, "0.3.0", now_unix()), Outcome::None);
        assert!(!crate::safe_fs::exists(&stale));
        assert!(crate::safe_fs::is_dir(&other));
        assert!(crate::safe_fs::is_file(
            &ups.join("00112233445566778899aabbccddeef0")
        ));
        // updates\ 不存在时什么都不做
        let empty = root.join("empty-root");
        assert_eq!(sweep_work_dirs(&empty), 0);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn reconcile_cleans_marker_and_work_dir() {
        let root = std::env::temp_dir().join(format!("ca-pending-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let work = updates_root(&root).join("fedcba9876543210fedcba9876543210");
        std::fs::create_dir_all(&work).unwrap();
        let inst = work.join("setup-0000000000000000.exe");
        crate::safe_fs::write_for_test(&inst, b"MZ").unwrap();
        let now = now_unix();
        write(
            &root,
            &marker("0.3.0", "0.3.1", now, &inst.to_string_lossy()),
        )
        .unwrap();
        assert_eq!(read(&root).unwrap().to, "0.3.1");

        assert_eq!(
            reconcile(&root, "0.3.1", now),
            Outcome::Updated(pair("0.3.0", "0.3.1"))
        );
        assert!(!crate::safe_fs::exists(&marker_path(&root)));
        assert!(!crate::safe_fs::exists(&work));
        // 第二次启动：没有标记，什么都不提示
        assert_eq!(reconcile(&root, "0.3.1", now), Outcome::None);

        // 无法解析的标记：删除，不提示
        crate::safe_fs::write_for_test(&marker_path(&root), b"{not json").unwrap();
        assert_eq!(reconcile(&root, "0.3.0", now), Outcome::None);
        assert!(!crate::safe_fs::exists(&marker_path(&root)));

        // 标记里写了工作目录以外的路径：只删标记，不碰那个目录
        let outside = root.join("keep");
        std::fs::create_dir_all(&outside).unwrap();
        write(
            &root,
            &marker(
                "0.3.0",
                "0.3.1",
                now,
                &outside.join("x.exe").to_string_lossy(),
            ),
        )
        .unwrap();
        assert_eq!(
            reconcile(&root, "0.3.0", now),
            Outcome::Unfinished(pair("0.3.0", "0.3.1"))
        );
        assert!(crate::safe_fs::is_dir(&outside));
        let _ = std::fs::remove_dir_all(&root);
    }
}
