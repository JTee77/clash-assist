//! Windows 安装链中的纯逻辑（windows-plan r4 §2.4，S-10 / S-6 / M-5）。
//!
//! FFI（注册表、`GetFinalPathNameByHandleW`、PE 版本资源、进程）在 `windows.rs`；
//! 这里只放判定，注入读取函数，在所有平台上单测。

// 非 Windows 只在单测中编译（判定逻辑全平台测），FFI 调用方不存在。
#![cfg_attr(not(windows), allow(dead_code))]

use std::time::Duration;

use super::InstallKind;

/// `HKCU\` 下 NSIS 安装版的 Uninstall 键（V8；模板 `UNINSTKEY`）。
pub const UNINSTALL_SUBKEY: &str =
    r"Software\Microsoft\Windows\CurrentVersion\Uninstall\Clash Assist";

/// 比较用的规范化：去掉 `\\?\` 前缀，`/` 换成 `\`，转小写（NTFS 路径不区分大小写）。
pub fn normalize_win_path(p: &str) -> String {
    let p = p.trim();
    let p = p
        .strip_prefix(r"\\?\UNC\")
        .map(|rest| format!(r"\\{rest}"))
        .unwrap_or_else(|| p.strip_prefix(r"\\?\").unwrap_or(p).to_string());
    p.replace('/', "\\").to_lowercase()
}

/// 去掉 Win32 扩展路径的 `\\?\` 前缀（仅盘符形式）。
///
/// `GetFinalPathNameByHandleW` 常返回 `\\?\C:\…`；生产侧 `is_file` 走 `safe_fs`，
/// 只接受 `X:\…`。若不剥前缀，NSIS 安装会被误判为 portable（#61 e2e）。
/// `\\?\UNC\…` 等非盘符形式原样返回。
pub fn strip_win32_verbatim_prefix(p: &str) -> &str {
    match p.strip_prefix(r"\\?\") {
        Some(rest)
            if rest.len() >= 2
                && rest.as_bytes()[0].is_ascii_alphabetic()
                && rest.as_bytes()[1] == b':' =>
        {
            rest
        }
        _ => p,
    }
}

fn parent_dir(p: &str) -> Option<&str> {
    let i = p.rfind(['\\', '/'])?;
    (i > 0).then(|| &p[..i])
}

/// `MainBinaryName` 只能是不带路径的 `*.exe` 文件名。
fn is_plain_exe_name(name: &str) -> bool {
    !name.is_empty()
        && !name.contains(['\\', '/', ':', '"', '\0'])
        && name != "."
        && name != ".."
        && name.to_ascii_lowercase().ends_with(".exe")
}

/// 安装类型判定（S-10）：
/// - 从 Uninstall 键读 `InstallLocation`（去掉引号）与 `MainBinaryName`，拼出期望的 exe 路径；
/// - 期望路径与当前 exe 都经 `final_path`（生产为 `GetFinalPathNameByHandleW`）规范化后比较，不区分大小写；
/// - 同目录下必须有 `uninstall.exe`；
/// - 其余情况一律 `Portable`。便携 zip 里的 `portable.txt` 不参与判定。
pub fn classify_install(
    install_location: Option<&str>,
    main_binary_name: Option<&str>,
    current_exe_final: Option<&str>,
    final_path: impl Fn(&str) -> Option<String>,
    is_file: impl Fn(&str) -> bool,
) -> InstallKind {
    let (Some(loc), Some(bin), Some(cur)) = (install_location, main_binary_name, current_exe_final)
    else {
        return InstallKind::Portable;
    };
    let loc = loc.trim().trim_matches('"').trim();
    let bin = bin.trim().trim_matches('"').trim();
    if loc.is_empty() || !is_plain_exe_name(bin) {
        return InstallKind::Portable;
    }
    let dir = loc.trim_end_matches(['\\', '/']);
    let expected = format!("{dir}\\{bin}");
    let Some(expected_final) = final_path(&expected) else {
        return InstallKind::Portable;
    };
    if normalize_win_path(&expected_final) != normalize_win_path(cur) {
        return InstallKind::Portable;
    }
    let Some(final_dir) = parent_dir(&expected_final) else {
        return InstallKind::Portable;
    };
    // GetFinalPathNameByHandleW 常返回 `\\?\C:\…`；生产 `is_file` 是 safe_fs，
    // 只接受 `X:\…`；不剥前缀则 NSIS 会被误判为 portable（#61 e2e）。
    let final_dir = strip_win32_verbatim_prefix(final_dir);
    if !is_file(&format!("{final_dir}\\uninstall.exe")) {
        return InstallKind::Portable;
    }
    InstallKind::Nsis
}

/// S-6：安装包 PE 版本资源里的 `ProductVersion` 必须等于目标版本（模板 `VIAddVersionKey "ProductVersion" "${VERSION}"`）。
pub fn check_product_version(found: Option<&str>, expected: &str) -> Result<(), String> {
    match found.map(|s| s.trim_end_matches('\0').trim()) {
        Some(v) if v == expected => Ok(()),
        Some(v) => Err(format!(
            "安装包版本资源与 release 不符（ProductVersion {v}，应为 {expected}），已拒绝安装"
        )),
        None => Err("安装包缺少 ProductVersion 版本资源，已拒绝安装".into()),
    }
}

/// 安装程序启动后的去向（M-5）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChildFate {
    /// 观察期内以 0 退出：视为完成（`/R` 已负责重启），本进程退出。
    ExitedOk,
    /// 观察期内以非 0 退出（例如降级被 Abort、被 SAC 拦截）：提示失败，不退出。
    ExitedErr(Option<i32>),
    /// 观察期结束仍在运行：本进程退出，交给安装程序（`RmShutdown` 本来也会结束本进程）。
    StillRunning,
}

/// 每 `step` 调用一次 `try_wait`，最多观察 `budget`（plan：100 ms × 2 s）。
/// `try_wait` 返回 `Ok(Some(code))` 表示已退出；查询出错按「仍在运行」处理（交给下次启动对账）。
pub fn observe_child(
    mut try_wait: impl FnMut() -> std::io::Result<Option<Option<i32>>>,
    budget: Duration,
    step: Duration,
    mut sleep: impl FnMut(Duration),
) -> ChildFate {
    let rounds = (budget.as_millis() / step.as_millis().max(1)).max(1) as u32;
    for _ in 0..rounds {
        sleep(step);
        match try_wait() {
            Ok(Some(Some(0))) => return ChildFate::ExitedOk,
            Ok(Some(code)) => return ChildFate::ExitedErr(code),
            Ok(None) | Err(_) => {}
        }
    }
    ChildFate::StillRunning
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXE: &str = r"C:\Users\me\AppData\Local\Clash Assist\clash-assist.exe";

    fn id(p: &str) -> Option<String> {
        Some(p.to_string())
    }

    #[test]
    fn nsis_requires_matching_paths_and_uninstaller() {
        let un = |p: &str| {
            normalize_win_path(p)
                == normalize_win_path(r"C:\Users\me\AppData\Local\Clash Assist\uninstall.exe")
        };
        let loc = r"C:\Users\me\AppData\Local\Clash Assist";
        // 模板写入的 InstallLocation 带引号
        let quoted = format!("\"{loc}\"");
        assert_eq!(
            classify_install(Some(&quoted), Some("clash-assist.exe"), Some(EXE), id, un),
            InstallKind::Nsis
        );
        // 不带引号、尾部反斜杠、大小写不同、\\?\ 前缀都视为同一路径
        assert_eq!(
            classify_install(
                Some(&format!("{loc}\\")),
                Some("Clash-Assist.EXE"),
                Some(EXE),
                id,
                un
            ),
            InstallKind::Nsis
        );
        let upper = EXE.to_uppercase();
        assert_eq!(
            classify_install(Some(loc), Some("clash-assist.exe"), Some(&upper), id, un),
            InstallKind::Nsis
        );
        let verbatim = format!(r"\\?\{EXE}");
        assert_eq!(
            classify_install(Some(loc), Some("clash-assist.exe"), Some(&verbatim), id, un),
            InstallKind::Nsis
        );
        // 缺 uninstall.exe → Portable
        assert_eq!(
            classify_install(Some(loc), Some("clash-assist.exe"), Some(EXE), id, |_| {
                false
            }),
            InstallKind::Portable
        );
        // 当前 exe 在别处（便携 zip 解压目录）→ Portable
        assert_eq!(
            classify_install(
                Some(loc),
                Some("clash-assist.exe"),
                Some(r"D:\tools\Clash Assist\clash-assist.exe"),
                id,
                |_| true
            ),
            InstallKind::Portable
        );
        // 注册表缺值 / 空值 / MainBinaryName 带路径 → Portable
        assert_eq!(
            classify_install(None, Some("clash-assist.exe"), Some(EXE), id, un),
            InstallKind::Portable
        );
        assert_eq!(
            classify_install(Some(loc), None, Some(EXE), id, un),
            InstallKind::Portable
        );
        assert_eq!(
            classify_install(Some("\"\""), Some("clash-assist.exe"), Some(EXE), id, un),
            InstallKind::Portable
        );
        assert_eq!(
            classify_install(
                Some(r"C:\Users\me\AppData\Local"),
                Some(r"Clash Assist\clash-assist.exe"),
                Some(EXE),
                id,
                un
            ),
            InstallKind::Portable
        );
        assert_eq!(
            classify_install(Some(loc), Some("clash-assist.bat"), Some(EXE), id, un),
            InstallKind::Portable
        );
        // 无法规范化（期望路径不存在 / 打不开）→ Portable
        assert_eq!(
            classify_install(Some(loc), Some("clash-assist.exe"), Some(EXE), |_| None, un),
            InstallKind::Portable
        );
        // 当前 exe 无法规范化 → Portable
        assert_eq!(
            classify_install(Some(loc), Some("clash-assist.exe"), None, id, un),
            InstallKind::Portable
        );
    }

    #[test]
    fn final_path_decides_through_links() {
        // InstallLocation 经过联接点：规范化后指向同一个真实路径即视为安装版；uninstall.exe 按真实目录找
        let real = r"E:\Real\Clash Assist\clash-assist.exe";
        let fp = |p: &str| {
            Some(if p.to_lowercase().starts_with(r"c:\link") {
                real.to_string()
            } else {
                p.to_string()
            })
        };
        let un = |p: &str| p == r"E:\Real\Clash Assist\uninstall.exe";
        assert_eq!(
            classify_install(
                Some(r"C:\Link\Clash Assist"),
                Some("clash-assist.exe"),
                Some(real),
                fp,
                un
            ),
            InstallKind::Nsis
        );
    }

    #[test]
    fn strip_win32_verbatim_prefix_drive_only() {
        assert_eq!(
            strip_win32_verbatim_prefix(r"\\?\C:\Users\me\clash-assist.exe"),
            r"C:\Users\me\clash-assist.exe"
        );
        assert_eq!(
            strip_win32_verbatim_prefix(r"\\?\d:\App\uninstall.exe"),
            r"d:\App\uninstall.exe"
        );
        // 无前缀 / UNC / 设备路径：原样返回
        assert_eq!(
            strip_win32_verbatim_prefix(r"C:\Users\me\clash-assist.exe"),
            r"C:\Users\me\clash-assist.exe"
        );
        assert_eq!(
            strip_win32_verbatim_prefix(r"\\?\UNC\server\share\file.exe"),
            r"\\?\UNC\server\share\file.exe"
        );
        assert_eq!(
            strip_win32_verbatim_prefix(r"\\?\Volume{00000000-0000-0000-0000-000000000000}\x"),
            r"\\?\Volume{00000000-0000-0000-0000-000000000000}\x"
        );
    }

    /// 回归：final_path（生产 = GetFinalPathNameByHandleW）返回 `\\?\C:\…`，
    /// 而生产 is_file（safe_fs）只接受非 verbatim 的 `X:\…`。旧代码会把
    /// uninstall.exe 检查喂成 `\\?\…\uninstall.exe` → 恒 false → Portable。
    #[test]
    fn verbatim_final_path_still_finds_uninstaller() {
        let loc = r"C:\Users\me\AppData\Local\Clash Assist";
        let verbatim_exe = format!(r"\\?\{EXE}");
        let fp = |p: &str| {
            Some(if p.starts_with(r"\\?\") {
                p.to_string()
            } else {
                format!(r"\\?\{p}")
            })
        };
        // 只对「已剥前缀」的 uninstall 路径返回 true（模拟 safe_fs 拒绝 `\\?\`）
        let un = |p: &str| p == r"C:\Users\me\AppData\Local\Clash Assist\uninstall.exe";
        assert_eq!(
            classify_install(
                Some(loc),
                Some("clash-assist.exe"),
                Some(&verbatim_exe),
                fp,
                un
            ),
            InstallKind::Nsis
        );
    }

    #[test]
    fn product_version_must_equal_target() {
        assert!(check_product_version(Some("0.3.1"), "0.3.1").is_ok());
        assert!(check_product_version(Some("0.3.1\0"), "0.3.1").is_ok());
        assert!(check_product_version(Some(" 0.3.1 "), "0.3.1").is_ok());
        assert!(check_product_version(Some("0.3.0"), "0.3.1").is_err());
        assert!(check_product_version(Some("0.3.1.0"), "0.3.1").is_err());
        assert!(check_product_version(Some(""), "0.3.1").is_err());
        assert!(check_product_version(None, "0.3.1").is_err());
    }

    #[test]
    fn observe_child_outcomes() {
        let step = Duration::from_millis(100);
        let budget = Duration::from_secs(2);
        let mut slept = Vec::new();
        // 第 3 次查询时以 0 退出
        let mut n = 0;
        let fate = observe_child(
            || {
                n += 1;
                Ok((n == 3).then_some(Some(0)))
            },
            budget,
            step,
            |d| slept.push(d),
        );
        assert_eq!(fate, ChildFate::ExitedOk);
        assert_eq!(slept.len(), 3);
        // 非 0 退出（降级 Abort 等）
        assert_eq!(
            observe_child(|| Ok(Some(Some(2))), budget, step, |_| {}),
            ChildFate::ExitedErr(Some(2))
        );
        // 被信号 / 无退出码
        assert_eq!(
            observe_child(|| Ok(Some(None)), budget, step, |_| {}),
            ChildFate::ExitedErr(None)
        );
        // 2 s 内一直在运行：恰好查询 20 次
        let mut calls = 0;
        assert_eq!(
            observe_child(
                || {
                    calls += 1;
                    Ok(None)
                },
                budget,
                step,
                |_| {}
            ),
            ChildFate::StillRunning
        );
        assert_eq!(calls, 20);
        // 查询出错按仍在运行处理
        assert_eq!(
            observe_child(|| Err(std::io::Error::other("x")), budget, step, |_| {}),
            ChildFate::StillRunning
        );
    }
}
