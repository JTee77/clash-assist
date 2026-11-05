//! macOS DMG 安装链（由 update_check.rs 迁移，windows-plan r4 §2.4；非 Windows 编译）。
//!
//! M5 起与 Windows 共用 `parse_release_asset`：资产名按 V8 精确匹配，要求 `immutable == true`（C-3），
//! 并核对下载字节数 == API 的 `size`。

use super::*;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// 从当前可执行文件路径推导所在的 `.app` 包根目录（如 `/Applications/Clash Assist.app`）。
pub fn bundle_path_from_exe(exe_path: &Path) -> Option<PathBuf> {
    let macos_dir = exe_path.parent()?;
    if macos_dir.file_name()?.to_str()? != "MacOS" {
        return None;
    }
    let contents_dir = macos_dir.parent()?;
    if contents_dir.file_name()?.to_str()? != "Contents" {
        return None;
    }
    let app_dir = contents_dir.parent()?;
    let app_name = app_dir.file_name()?.to_str()?;
    if !app_name.ends_with(".app") {
        return None;
    }
    Some(app_dir.to_path_buf())
}

/// 解析自更新要覆盖的目标 `.app` 路径：
/// - 若在只读挂载卷 `/Volumes/...` 内运行，则安装到 `/Applications/Clash Assist.app`；
/// - 若在正常 `.app` 包内运行，则原位覆盖该 `.app`；
/// - 开发模式下若未指定 `CLASH_ASSIST_UPDATE_TARGET_APP`，拒绝覆盖并返回提示。
pub fn resolve_target_app_bundle(exe_path: &Path) -> Result<PathBuf, String> {
    if let Ok(custom) = std::env::var("CLASH_ASSIST_UPDATE_TARGET_APP") {
        let trimmed = custom.trim();
        if !trimmed.is_empty() {
            return Ok(PathBuf::from(trimmed));
        }
    }
    if let Some(bundle) = bundle_path_from_exe(exe_path) {
        if bundle.starts_with("/Volumes/") {
            return Ok(PathBuf::from("/Applications/Clash Assist.app"));
        }
        return Ok(bundle);
    }
    Err("当前为开发模式运行（非 .app 应用包），无法原地替换；请前往下载页更新".into())
}

/// 进程内计算并校验下载的 DMG 文件 SHA-256。fail closed：未提供摘要或摘要格式不合法时一律报错。
pub fn verify_sha256_file(path: &Path, expected_digest: Option<&str>) -> Result<(), String> {
    let Some(raw) = expected_digest.map(str::trim).filter(|s| !s.is_empty()) else {
        return Err("缺少安装包 SHA-256 摘要，拒绝安装未校验的更新包".into());
    };
    let expected =
        normalize_sha256_digest(raw).ok_or_else(|| format!("无效的 SHA-256 摘要格式：{raw}"))?;
    let mut file = crate::safe_fs::open_read(path).map_err(|e| format!("打开安装包失败：{e}"))?;
    let actual = sha256_of_reader(&mut file).map_err(|e| format!("读取安装包失败：{e}"))?;
    if actual != expected {
        return Err(format!(
            "安装包 SHA-256 校验不匹配（期望 {expected}，实际 {actual}）"
        ));
    }
    Ok(())
}

struct MountGuard {
    mount_point: PathBuf,
    active: bool,
}

impl MountGuard {
    fn detach_now(&mut self) {
        if self.active {
            let _ = Command::new("/usr/bin/hdiutil")
                .args(["detach", "-force"])
                .arg(&self.mount_point)
                .output();
            self.active = false;
        }
    }
}

impl Drop for MountGuard {
    fn drop(&mut self) {
        self.detach_now();
    }
}

fn find_app_in_dir(dir: &Path) -> Option<PathBuf> {
    let preferred = dir.join("Clash Assist.app");
    if crate::safe_fs::is_dir(&preferred) {
        return Some(preferred);
    }
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let p = entry.path();
        if crate::safe_fs::is_dir(&p) {
            if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
                if name.ends_with(".app") {
                    return Some(p);
                }
            }
        }
    }
    None
}

fn copy_bundle_via_ditto(src: &Path, dst: &Path) -> Result<(), String> {
    if crate::safe_fs::exists(dst) {
        std::fs::remove_dir_all(dst).map_err(|e| format!("清理旧目录失败：{e}"))?;
    }
    let out = Command::new("/usr/bin/ditto")
        .arg(src)
        .arg(dst)
        .output()
        .map_err(|e| format!("执行 ditto 失败：{e}"))?;
    if !out.status.success() {
        return Err(format!(
            "复制应用包失败：{}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(())
}

fn move_or_ditto_bundle(src: &Path, dst: &Path) -> Result<(), String> {
    if crate::safe_fs::exists(dst) {
        std::fs::remove_dir_all(dst).map_err(|e| format!("移除旧目录失败：{e}"))?;
    }
    if std::fs::rename(src, dst).is_ok() {
        return Ok(());
    }
    copy_bundle_via_ditto(src, dst)?;
    let _ = std::fs::remove_dir_all(src);
    Ok(())
}

/// 静默挂载 `.dmg`，提取 `.app`，清除 `com.apple.quarantine` 隔离属性，校验签名并原子替换 `target_app`。
pub fn apply_dmg_to_app_bundle(
    dmg_path: &Path,
    target_app: &Path,
    work_dir: &Path,
) -> Result<(), String> {
    let mnt_dir = work_dir.join("mnt");
    std::fs::create_dir_all(&mnt_dir).map_err(|e| format!("创建挂载目录失败：{e}"))?;

    let attach = Command::new("/usr/bin/hdiutil")
        .args([
            "attach",
            "-nobrowse",
            "-readonly",
            "-noverify",
            "-noautoopen",
            "-mountpoint",
        ])
        .arg(&mnt_dir)
        .arg(dmg_path)
        .output()
        .map_err(|e| format!("调用 hdiutil 失败：{e}"))?;
    if !attach.status.success() {
        return Err(format!(
            "挂载更新镜像失败：{}",
            String::from_utf8_lossy(&attach.stderr).trim()
        ));
    }

    let mut mount_guard = MountGuard {
        mount_point: mnt_dir.clone(),
        active: true,
    };

    let mounted_app =
        find_app_in_dir(&mnt_dir).ok_or_else(|| "更新镜像中未找到 .app 应用包".to_string())?;
    let staging_app = work_dir.join("staging.app");
    copy_bundle_via_ditto(&mounted_app, &staging_app)?;

    // 复制完成后立即卸载 DMG
    mount_guard.detach_now();

    // 清除全部扩展属性（尤其是 com.apple.quarantine），确保重启后绝不触发 Gatekeeper 拦截弹窗
    let _ = Command::new("/usr/bin/xattr")
        .arg("-cr")
        .arg(&staging_app)
        .output();

    // 替换前严格校验新版代码签名
    let verify = Command::new("/usr/bin/codesign")
        .args(["--verify", "--deep", "--strict"])
        .arg(&staging_app)
        .output()
        .map_err(|e| format!("调用 codesign 失败：{e}"))?;
    if !verify.status.success() {
        return Err(format!(
            "更新包代码签名校验未通过：{}",
            String::from_utf8_lossy(&verify.stderr).trim()
        ));
    }

    let backup_app = work_dir.join("backup.app");
    let had_existing = crate::safe_fs::exists(target_app);
    if had_existing {
        move_or_ditto_bundle(target_app, &backup_app)
            .map_err(|e| format!("备份当前应用失败（可能缺少写入权限）：{e}"))?;
    }

    if let Err(e) = move_or_ditto_bundle(&staging_app, target_app) {
        if had_existing && crate::safe_fs::exists(&backup_app) {
            let _ = move_or_ditto_bundle(&backup_app, target_app);
        }
        return Err(format!("写入新版本应用失败：{e}"));
    }

    // 对最终落盘目录再清一次扩展属性，确保万无一失
    let _ = Command::new("/usr/bin/xattr")
        .arg("-cr")
        .arg(target_app)
        .output();

    Ok(())
}

fn staging_base_dir() -> PathBuf {
    let t = std::env::temp_dir();
    if crate::safe_fs::is_dir(&t) {
        t
    } else {
        PathBuf::from("/tmp")
    }
}

/// 阶段 1（下载）：流式下载最新 `.dmg` 到独立临时目录（进度经回调上报，lib.rs 负责 emit），完成后做 SHA-256 校验，返回临时工作目录路径。
///
/// 下载地址与期望摘要只由 Rust 侧重新查询 GitHub release 元数据得到（[`resolve_download_target`]），
/// 不接受前端传入；release 未提供摘要时 fail closed，引导用户去下载页手动更新。
pub async fn download_to_staging(on_progress: ProgressCallback<'_>) -> Result<String, String> {
    let exe = std::env::current_exe().map_err(|e| format!("无法定位当前程序路径：{e}"))?;
    let _target_app = resolve_target_app_bundle(&exe)?;

    let info = check(env!("CARGO_PKG_VERSION")).await?;
    let asset = resolve_download_target(&info)?;
    if asset.kind != AssetKind::MacDmgAarch64 {
        return Err("最新 Release 中没有 macOS 安装包".into());
    }

    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let work_dir = staging_base_dir().join(format!(
        "clash-assist-update-{}-{now_ms}",
        std::process::id()
    ));
    std::fs::create_dir_all(&work_dir).map_err(|e| format!("创建临时更新目录失败：{e}"))?;

    let dmg_path = work_dir.join("update.dmg");
    let proxy_port = active_local_proxy_port_async().await;
    if let Err(e) = stream_asset_to_file(
        &asset.url,
        env!("CARGO_PKG_VERSION"),
        &dmg_path,
        proxy_port,
        on_progress,
        DownloadLimits {
            max_bytes: asset.kind.max_bytes(),
            expected_size: Some(asset.size),
        },
    )
    .await
    {
        let _ = std::fs::remove_dir_all(&work_dir);
        return Err(format!("下载更新包失败：{e}"));
    }

    let dmg_for_verify = dmg_path.clone();
    let expected_sha = asset.sha256.clone();
    let verify_res = tauri::async_runtime::spawn_blocking(move || {
        verify_sha256_file(&dmg_for_verify, Some(&expected_sha))
    })
    .await
    .map_err(|e| format!("校验任务异常终止：{e}"))?;

    if let Err(e) = verify_res {
        let _ = std::fs::remove_dir_all(&work_dir);
        return Err(e);
    }

    Ok(work_dir.to_string_lossy().to_string())
}

/// 阶段 2（后台静默安装）：从临时工作目录挂载 `update.dmg`，清除隔离属性并替换当前 `.app`（不自动退出重启）。
pub async fn apply_staged_update(work_dir_str: String) -> Result<String, String> {
    let exe = std::env::current_exe().map_err(|e| format!("无法定位当前程序路径：{e}"))?;
    let target_app = resolve_target_app_bundle(&exe)?;

    let work_dir = PathBuf::from(work_dir_str.trim());
    let file_name = work_dir.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if !file_name.starts_with("clash-assist-update-") || !crate::safe_fs::is_dir(&work_dir) {
        return Err("无效的临时更新目录".into());
    }
    // 只接受 download_to_staging 建在临时目录根下的工作目录（防止前端指向别处同名目录）。
    let parent_ok = work_dir
        .parent()
        .is_some_and(|p| crate::safe_fs::same_dir(p, &staging_base_dir()));
    if !parent_ok {
        return Err("无效的临时更新目录".into());
    }
    let dmg_path = work_dir.join("update.dmg");
    if !crate::safe_fs::is_file(&dmg_path) {
        let _ = std::fs::remove_dir_all(&work_dir);
        return Err("未找到已下载的更新包文件".into());
    }

    let target_for_task = target_app.clone();
    let work_for_task = work_dir.clone();
    let apply_res = tauri::async_runtime::spawn_blocking(move || {
        apply_dmg_to_app_bundle(&dmg_path, &target_for_task, &work_for_task)
    })
    .await
    .map_err(|e| format!("安装任务异常终止：{e}"))?;

    let _ = std::fs::remove_dir_all(&work_dir);
    apply_res?;

    Ok(target_app.to_string_lossy().to_string())
}

/// 阶段 3（用户主动点击「立即重启」）：拉起已替换的新版 `.app` 并退出当前进程。
pub fn relaunch_updated_app() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("无法定位当前程序路径：{e}"))?;
    let target_app = resolve_target_app_bundle(&exe)?;
    if std::env::var("CLASH_ASSIST_NO_RELAUNCH").ok().as_deref() != Some("1") {
        Command::new("/bin/sh")
            .arg("-c")
            .arg("sleep 0.45 && /usr/bin/open -n \"$1\"")
            .arg("sh")
            .arg(&target_app)
            .spawn()
            .map_err(|e| format!("启动新版本失败：{e}"))?;
        std::thread::spawn(|| {
            std::thread::sleep(Duration::from_millis(150));
            std::process::exit(0);
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD_SHA: &str =
        "sha256:251112b7cd87706978a3ad57b15b3d4191dfacc65fefa8d9b5c36da01482dbdc";

    #[test]
    fn verify_sha256_file_fails_closed_and_matches() {
        let dir =
            std::env::temp_dir().join(format!("clash-assist-sha-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("x.bin");
        crate::safe_fs::write_for_test(&f, b"abc").unwrap();
        // sha256("abc")
        let abc = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        assert!(verify_sha256_file(&f, None).is_err());
        assert!(verify_sha256_file(&f, Some("")).is_err());
        assert!(verify_sha256_file(&f, Some("sha256:nothex")).is_err());
        assert!(verify_sha256_file(&f, Some(GOOD_SHA)).is_err());
        assert!(verify_sha256_file(&f, Some(abc)).is_ok());
        assert!(verify_sha256_file(&f, Some(&format!("sha256:{}", abc.to_uppercase()))).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bundle_path_detection() {
        let p = Path::new("/Applications/Clash Assist.app/Contents/MacOS/clash-assist");
        assert_eq!(
            bundle_path_from_exe(p),
            Some(PathBuf::from("/Applications/Clash Assist.app"))
        );
        let vol = Path::new("/Volumes/Clash Assist/Clash Assist.app/Contents/MacOS/clash-assist");
        assert_eq!(
            resolve_target_app_bundle(vol).unwrap(),
            PathBuf::from("/Applications/Clash Assist.app")
        );
        let dev =
            Path::new("/Users/test/Projects/clash-assist/src-tauri/target/debug/clash-assist");
        assert!(bundle_path_from_exe(dev).is_none());
    }

    /// S-4：hdiutil 已知的瞬时错误（按输出文本匹配，不区分大小写）。模式是假设：
    /// 取自 hdiutil 常见报错（如 `hdiutil: create failed - Resource busy`），未在本仓库 CI 日志中见到实例。
    const HDIUTIL_TRANSIENT: &[&str] = &["resource busy", "resource temporarily unavailable"];
    /// 失败后的退避：最多 3 次尝试，间隔 5 s、15 s。
    #[cfg(target_os = "macos")]
    const HDIUTIL_BACKOFF: &[Duration] = &[Duration::from_secs(5), Duration::from_secs(15)];

    fn is_transient_hdiutil_error(output: &str) -> bool {
        let o = output.to_ascii_lowercase();
        HDIUTIL_TRANSIENT.iter().any(|p| o.contains(p))
    }

    /// 运行 `run`；失败且输出含已知瞬时错误时按 `HDIUTIL_BACKOFF` 重试，其余失败立即返回。
    #[cfg(target_os = "macos")]
    fn hdiutil_with_retry(run: impl Fn() -> std::process::Output) -> std::process::Output {
        let mut backoff = HDIUTIL_BACKOFF.iter();
        loop {
            let out = run();
            if out.status.success() {
                return out;
            }
            let text = format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            match backoff.next() {
                Some(wait) if is_transient_hdiutil_error(&text) => {
                    eprintln!("hdiutil 瞬时错误，{wait:?} 后重试：{}", text.trim());
                    std::thread::sleep(*wait);
                }
                _ => return out,
            }
        }
    }

    #[test]
    fn hdiutil_transient_error_patterns() {
        assert!(is_transient_hdiutil_error(
            "hdiutil: create failed - Resource busy"
        ));
        assert!(is_transient_hdiutil_error(
            "hdiutil: attach failed - Resource temporarily unavailable"
        ));
        assert!(!is_transient_hdiutil_error(
            "hdiutil: create failed - No space left on device"
        ));
        assert!(!is_transient_hdiutil_error(
            "hdiutil: create failed - Permission denied"
        ));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn end_to_end_apply_dmg_replaces_app_and_strips_quarantine() {
        let now_ns = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let base = PathBuf::from("/tmp").join(format!(
            "clash-assist-self-update-test-{}-{now_ns}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        let src_dir = base.join("dmg_src");
        let new_app = src_dir.join("Clash Assist.app");
        let macos_dir = new_app.join("Contents/MacOS");
        std::fs::create_dir_all(&macos_dir).unwrap();

        let plist = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleExecutable</key>
  <string>clash-assist</string>
  <key>CFBundleIdentifier</key>
  <string>io.github.jtee77.clash-assist.test</string>
  <key>CFBundleName</key>
  <string>Clash Assist</string>
  <key>CFBundleVersion</key>
  <string>0.2.5</string>
  <key>CFBundleShortVersionString</key>
  <string>0.2.5</string>
</dict>
</plist>"#;
        crate::safe_fs::write_for_test(&new_app.join("Contents/Info.plist"), plist.as_bytes())
            .unwrap();
        let c_src = base.join("main.c");
        crate::safe_fs::write_for_test(&c_src, b"int main(void) { return 0; }\n").unwrap();
        let cc = Command::new("/usr/bin/cc")
            .env("TMPDIR", "/tmp")
            .arg(&c_src)
            .arg("-o")
            .arg(macos_dir.join("clash-assist"))
            .output()
            .unwrap();
        assert!(cc.status.success(), "cc failed");

        // Ad-hoc sign the bundle and deliberately attach a com.apple.quarantine xattr
        let sign = Command::new("/usr/bin/codesign")
            .args(["--force", "--deep", "--sign", "-"])
            .arg(&new_app)
            .output()
            .unwrap();
        assert!(sign.status.success(), "codesign failed");

        let _ = Command::new("/usr/bin/xattr")
            .args(["-w", "com.apple.quarantine", "0081;00000000;Safari;"])
            .arg(&new_app)
            .output();

        // Create a real DMG containing the signed app
        let dmg_path = base.join("Clash.Assist_0.2.5_aarch64.dmg");
        // S-4：CI 上 hdiutil 偶发「Resource busy」，只对已知的瞬时错误有限重试。
        let create_dmg = hdiutil_with_retry(|| {
            Command::new("/usr/bin/hdiutil")
                .args(["create", "-volname", "Clash Assist", "-srcfolder"])
                .arg(&src_dir)
                .args(["-ov", "-format", "UDZO"])
                .arg(&dmg_path)
                .output()
                .unwrap()
        });
        assert!(
            create_dmg.status.success(),
            "hdiutil create failed: {}",
            String::from_utf8_lossy(&create_dmg.stderr)
        );

        // Compute real SHA-256 and test verify_sha256_file
        let sha_out = Command::new("/usr/bin/shasum")
            .args(["-a", "256"])
            .arg(&dmg_path)
            .output()
            .unwrap();
        let sha_hex = String::from_utf8_lossy(&sha_out.stdout)
            .split_whitespace()
            .next()
            .unwrap()
            .to_string();
        assert!(verify_sha256_file(&dmg_path, Some(&format!("sha256:{sha_hex}"))).is_ok());
        assert!(verify_sha256_file(
            &dmg_path,
            Some("sha256:0000000000000000000000000000000000000000000000000000000000000000")
        )
        .is_err());

        // Prepare an old installed app to be replaced
        let target_app = base.join("Installed/Clash Assist.app");
        std::fs::create_dir_all(target_app.join("Contents")).unwrap();
        crate::safe_fs::write_for_test(&target_app.join("Contents/old.txt"), b"old-0.2.4").unwrap();

        let work_dir = base.join("work");
        std::fs::create_dir_all(&work_dir).unwrap();

        apply_dmg_to_app_bundle(&dmg_path, &target_app, &work_dir)
            .expect("apply_dmg should succeed");

        assert!(!crate::safe_fs::exists(
            &target_app.join("Contents/old.txt")
        ));
        let new_plist =
            crate::safe_fs::read_to_string(&target_app.join("Contents/Info.plist")).unwrap();
        assert!(new_plist.contains("0.2.5"));

        let verify = Command::new("/usr/bin/codesign")
            .args(["--verify", "--deep", "--strict"])
            .arg(&target_app)
            .output()
            .unwrap();
        assert!(verify.status.success());

        let xattr_out = Command::new("/usr/bin/xattr")
            .arg("-l")
            .arg(&target_app)
            .output()
            .unwrap();
        let xattrs = String::from_utf8_lossy(&xattr_out.stdout);
        assert!(
            !xattrs.contains("com.apple.quarantine"),
            "com.apple.quarantine must be stripped, got: {xattrs}"
        );

        let _ = std::fs::remove_dir_all(&base);
    }
}
