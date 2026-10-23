//! 更新检查与应用内分阶段自更新：
//! 1) `check`：查询 GitHub latest release，与当前版本做 semver 比较（含 .dmg 资产直链与 SHA-256 摘要）。
//! 2) `download_to_staging`：用户点击「一键更新」后，下载最新 .dmg 并校验 SHA-256。
//! 3) `apply_staged_update`：后台静默挂载 .dmg、提取 .app、清除 com.apple.quarantine 隔离属性、
//!    校验签名、原子替换当前 .app（替换后不强行退出，当前窗口可继续正常使用，下次重开自动生效）。
//! 4) `relaunch_app`：用户点击「立即重启」时，拉起新版 .app 并退出当前进程。

use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const LATEST_RELEASE_API: &str =
    "https://api.github.com/repos/JTee77/clash-assist/releases/latest";
pub const RELEASES_PAGE: &str = "https://github.com/JTee77/clash-assist/releases";
pub const DOWNLOAD_PREFIX: &str = "https://github.com/JTee77/clash-assist/releases/download/";
pub const FAKE_LATEST_ENV: &str = "CLASH_ASSIST_FAKE_LATEST";
const TIMEOUT_MS: u64 = 5000;
/// 下载进度事件名（前端 TitleBar 监听）。
pub const DOWNLOAD_PROGRESS_EVENT: &str = "update-download-progress";
/// 单块空闲超时：30 秒没有新数据视为连接停滞。替代旧的全局 120s 总超时——
/// 大安装包在慢网络下会被总超时误杀；流式下载有进度事件，停滞对用户可见。
const DOWNLOAD_IDLE_TIMEOUT_MS: u64 = 30_000;

/// 下载进度（camelCase 序列化给前端）。
#[derive(Debug, Clone, Serialize)]
pub struct DownloadProgress {
    pub downloaded: u64,
    pub total: Option<u64>,
    pub percent: Option<u8>,
}

#[derive(Debug, Serialize, Clone)]
pub struct UpdateInfo {
    pub current: String,
    pub latest: String,
    pub update_available: bool,
    pub url: String,
    /// true = 来自 CLASH_ASSIST_FAKE_LATEST（未访问网络）
    pub fake: bool,
    /// GitHub Release 中的 macOS .dmg 下载直链（若存在）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dmg_url: Option<String>,
    /// GitHub Release 中的 .dmg SHA-256 摘要（形如 "sha256:..."，若存在）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dmg_sha256: Option<String>,
}

/// "v1.2.3" / "1.2.3-beta.1" → (1,2,3, 是否预发布)。不合法返回 None。
pub fn parse_semver(s: &str) -> Option<(u64, u64, u64, bool)> {
    let s = s.trim().trim_start_matches(['v', 'V']);
    let (core, pre) = match s.split_once(['-', '+']) {
        Some((c, rest)) => (c, !rest.is_empty() && s.contains('-')),
        None => (s, false),
    };
    let mut it = core.split('.');
    let a = it.next()?.parse().ok()?;
    let b = it.next()?.parse().ok()?;
    let c = it.next()?.parse().ok()?;
    if it.next().is_some() {
        return None;
    }
    Some((a, b, c, pre))
}

/// latest 是否严格新于 current。同版本号时正式版 > 预发布版。
pub fn is_newer(latest: &str, current: &str) -> bool {
    match (parse_semver(latest), parse_semver(current)) {
        (Some((a, b, c, lp)), Some((x, y, z, cp))) => {
            let l = (a, b, c);
            let r = (x, y, z);
            l > r || (l == r && cp && !lp)
        }
        _ => false,
    }
}

fn normalize(v: &str) -> String {
    v.trim().trim_start_matches(['v', 'V']).to_string()
}

/// 校验 DMG 下载地址：只允许来自本项目 GitHub Releases 下载前缀且以 `.dmg` 结尾。
pub fn validate_dmg_url(url: &str) -> Result<(), String> {
    let trimmed = url.trim();
    if !trimmed.starts_with(DOWNLOAD_PREFIX) || !trimmed.to_ascii_lowercase().ends_with(".dmg") {
        return Err("只允许下载本项目 GitHub Releases 下的 .dmg 安装包".into());
    }
    Ok(())
}

/// 从 GitHub Release JSON 的 `assets` 数组中提取 `.dmg` 下载直链与可选的 `sha256:...` 摘要。
pub fn parse_release_dmg_asset(v: &serde_json::Value) -> (Option<String>, Option<String>) {
    let Some(assets) = v["assets"].as_array() else {
        return (None, None);
    };
    for asset in assets {
        let name = asset["name"].as_str().unwrap_or("");
        if !name.to_ascii_lowercase().ends_with(".dmg") {
            continue;
        }
        let dl = asset["browser_download_url"]
            .as_str()
            .or_else(|| asset["url"].as_str())
            .unwrap_or("");
        if validate_dmg_url(dl).is_ok() {
            let digest = asset["digest"]
                .as_str()
                .map(str::trim)
                .filter(|d| !d.is_empty())
                .map(str::to_string);
            return (Some(dl.to_string()), digest);
        }
    }
    (None, None)
}

/// 检测本机可能正在监听的代理混合端口（Verge 7897 / FlClash 7890），用于加速 GitHub API 与 Release 下载。
fn active_local_proxy_port() -> Option<u16> {
    use std::net::{SocketAddr, TcpStream};
    let mut candidates = vec![7897u16, 7890u16];
    let verge = crate::mihomo::discover_for_client("verge").mixed_port;
    if verge > 0 && !candidates.contains(&verge) {
        candidates.insert(0, verge);
    }
    let flclash = crate::mihomo::discover_for_client("flclash").mixed_port;
    if flclash > 0 && !candidates.contains(&flclash) {
        candidates.push(flclash);
    }
    for port in candidates {
        let addr = SocketAddr::from(([127, 0, 0, 1], port));
        if TcpStream::connect_timeout(&addr, Duration::from_millis(40)).is_ok() {
            return Some(port);
        }
    }
    None
}

fn build_http_client(
    current: &str,
    timeout_ms: Option<u64>,
    connect_ms: u64,
    proxy_port: Option<u16>,
) -> Result<reqwest::Client, String> {
    let mut builder = reqwest::Client::builder()
        .connect_timeout(Duration::from_millis(connect_ms))
        .user_agent(format!("clash-assist/{current}"));
    if let Some(ms) = timeout_ms {
        builder = builder.timeout(Duration::from_millis(ms));
    }
    if let Some(port) = proxy_port {
        let proxy = reqwest::Proxy::all(format!("http://127.0.0.1:{port}"))
            .map_err(|e| format!("proxy: {e}"))?;
        builder = builder.proxy(proxy);
    }
    builder.build().map_err(|e| format!("http client: {e}"))
}

async fn get_bytes_with_proxy_fallback(
    url: &str,
    current: &str,
    timeout_ms: u64,
    connect_ms: u64,
    accept_header: Option<&str>,
) -> Result<Vec<u8>, String> {
    if let Some(port) = active_local_proxy_port() {
        if let Ok(client) = build_http_client(current, Some(timeout_ms), connect_ms, Some(port)) {
            let mut req = client.get(url);
            if let Some(acc) = accept_header {
                req = req.header("Accept", acc);
            }
            if let Ok(resp) = req.send().await {
                if resp.status().is_success() {
                    if let Ok(bytes) = resp.bytes().await {
                        return Ok(bytes.to_vec());
                    }
                }
            }
        }
    }

    let client = build_http_client(current, Some(timeout_ms), connect_ms, None)?;
    let mut req = client.get(url);
    if let Some(acc) = accept_header {
        req = req.header("Accept", acc);
    }
    let resp = req
        .send()
        .await
        .map_err(|e| format!("请求 GitHub 失败：{e}"))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(format!("GitHub 返回 HTTP {}", status.as_u16()));
    }
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("读取 GitHub 响应失败：{e}"))?;
    Ok(bytes.to_vec())
}

/// 进度回调接缝：每跨过 1%（无总长时每 1 MiB）调用一次，收尾必补发 100%。
/// 生产路径由 lib.rs 传入「emit 到前端」的闭包；单测传入收集闭包——进度管线可无 GUI 全测。
pub type ProgressCallback<'a> = &'a (dyn Fn(DownloadProgress) + Send + Sync);

/// 流式下载 .dmg 到落盘：按块写入并回调进度，避免整包读进内存（旧实现
/// `resp.bytes()` 会把上百 MB 的 DMG 完整驻留 RAM）。只在「尚未开始传输」时做
/// 代理→直连回落；开始收流后中途失败直接报错，重试由用户再点一次。
async fn stream_dmg_to_file(
    url: &str,
    current: &str,
    dest: &Path,
    proxy_port: Option<u16>,
    on_progress: ProgressCallback<'_>,
) -> Result<(), String> {
    use futures_util::StreamExt;
    use std::io::Write;

    let mut last_err = String::new();
    for attempt_port in [proxy_port, None] {
        let client = match build_http_client(current, None, 5000, attempt_port) {
            Ok(c) => c,
            Err(e) => {
                last_err = e;
                continue;
            }
        };
        let resp = match client.get(url).send().await {
            Ok(r) if r.status().is_success() => r,
            Ok(r) => {
                last_err = format!("HTTP {}", r.status());
                continue;
            }
            Err(e) => {
                last_err = format!("连接失败：{e}");
                continue;
            }
        };
        let total = resp.content_length();
        let mut file = match std::fs::File::create(dest) {
            Ok(f) => f,
            Err(e) => return Err(format!("创建下载文件失败：{e}")),
        };
        let mut stream = resp.bytes_stream();
        let mut downloaded: u64 = 0;
        let mut last_pct: i64 = -1;
        let mut last_emitted_at: u64 = 0;
        loop {
            let next = match tokio::time::timeout(
                Duration::from_millis(DOWNLOAD_IDLE_TIMEOUT_MS),
                stream.next(),
            )
            .await
            {
                Ok(item) => item,
                Err(_) => {
                    return Err(format!(
                        "下载停滞：超过 {} 秒没有收到新数据，已中止",
                        DOWNLOAD_IDLE_TIMEOUT_MS / 1000
                    ));
                }
            };
            let Some(chunk) = next else { break };
            let chunk = chunk.map_err(|e| format!("读取下载流失败：{e}"))?;
            // 块为 KB 级，本地写入的短阻塞可忽略，不为此引入异步文件 IO。
            file.write_all(&chunk)
                .map_err(|e| format!("写入下载文件失败：{e}"))?;
            downloaded += chunk.len() as u64;

            let percent = total.map(|t| ((downloaded * 100) / t).min(100) as u8);
            let should_emit = match percent {
                Some(p) => (p as i64) != last_pct,
                None => downloaded - last_emitted_at >= (1 << 20), // 无总长：每 1 MiB 报一次
            };
            if should_emit {
                on_progress(DownloadProgress {
                    downloaded,
                    total,
                    percent,
                });
                if let Some(p) = percent {
                    last_pct = p as i64;
                }
                last_emitted_at = downloaded;
            }
        }
        file.flush().map_err(|e| format!("落盘失败：{e}"))?;
        // 收尾补发一次，确保前端能看到 100%（流末尾可能不足一个百分比间隔）
        if let Some(t) = total {
            let p = ((downloaded * 100) / t).min(100) as u8;
            if (p as i64) != last_pct {
                on_progress(DownloadProgress {
                    downloaded,
                    total,
                    percent: Some(p),
                });
            }
        }
        return Ok(());
    }
    Err(if last_err.is_empty() {
        "无可用的下载路径".to_string()
    } else {
        last_err
    })
}

pub async fn check(current: &str) -> Result<UpdateInfo, String> {
    if let Ok(fake) = std::env::var(FAKE_LATEST_ENV) {
        let fake = normalize(&fake);
        if !fake.is_empty() {
            if parse_semver(&fake).is_none() {
                return Err(format!("{FAKE_LATEST_ENV} 不是合法版本号：{fake}"));
            }
            return Ok(UpdateInfo {
                current: current.to_string(),
                update_available: is_newer(&fake, current),
                url: format!("{RELEASES_PAGE}/tag/v{fake}"),
                dmg_url: Some(format!(
                    "{DOWNLOAD_PREFIX}v{fake}/Clash.Assist_{fake}_aarch64.dmg"
                )),
                dmg_sha256: None,
                latest: fake,
                fake: true,
            });
        }
    }
    let bytes = get_bytes_with_proxy_fallback(
        LATEST_RELEASE_API,
        current,
        TIMEOUT_MS,
        3000,
        Some("application/vnd.github+json"),
    )
    .await?;
    let v: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|e| format!("解析 GitHub 响应失败：{e}"))?;
    let tag = v["tag_name"].as_str().ok_or("GitHub 响应缺少 tag_name")?;
    let latest = normalize(tag);
    if parse_semver(&latest).is_none() {
        return Err(format!("最新 release 的 tag 不是版本号：{tag}"));
    }
    let url = v["html_url"]
        .as_str()
        .filter(|u| u.starts_with("https://github.com/"))
        .map(str::to_string)
        .unwrap_or_else(|| format!("{RELEASES_PAGE}/tag/{tag}"));
    let (dmg_url, dmg_sha256) = parse_release_dmg_asset(&v);
    Ok(UpdateInfo {
        current: current.to_string(),
        update_available: is_newer(&latest, current),
        latest,
        url,
        fake: false,
        dmg_url,
        dmg_sha256,
    })
}

/// 只允许打开本项目的 GitHub release 页面（防止前端被诱导打开任意 URL）。
pub fn open_release_page(url: &str) -> Result<(), String> {
    if !url.starts_with(RELEASES_PAGE) {
        return Err("只允许打开本项目的 release 页面".into());
    }
    #[cfg(target_os = "macos")]
    let mut cmd = Command::new("open");
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut cmd = Command::new("xdg-open");
    #[cfg(windows)]
    let mut cmd = {
        let mut c = Command::new("cmd");
        c.args(["/C", "start", ""]);
        c
    };
    cmd.arg(url).spawn().map(|_| ()).map_err(|e| format!("打开浏览器失败：{e}"))
}

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

/// 使用系统 `/usr/bin/shasum -a 256` 校验下载的 DMG 文件完整性（若 GitHub Release 提供了 `sha256:...` 摘要）。
pub fn verify_sha256_file(path: &Path, expected_digest: Option<&str>) -> Result<(), String> {
    let Some(raw) = expected_digest.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(());
    };
    let expected = raw
        .strip_prefix("sha256:")
        .or_else(|| raw.strip_prefix("SHA256:"))
        .unwrap_or(raw)
        .trim()
        .to_ascii_lowercase();
    if expected.len() != 64 || !expected.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!("无效的 SHA-256 摘要格式：{raw}"));
    }
    let out = Command::new("/usr/bin/shasum")
        .args(["-a", "256"])
        .arg(path)
        .output()
        .map_err(|e| format!("调用 shasum 失败：{e}"))?;
    if !out.status.success() {
        return Err("计算安装包 SHA-256 失败".into());
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let actual = stdout
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
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
    if preferred.is_dir() {
        return Some(preferred);
    }
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
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
    if dst.exists() {
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
    if dst.exists() {
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

    let mounted_app = find_app_in_dir(&mnt_dir)
        .ok_or_else(|| "更新镜像中未找到 .app 应用包".to_string())?;
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
    let had_existing = target_app.exists();
    if had_existing {
        move_or_ditto_bundle(target_app, &backup_app)
            .map_err(|e| format!("备份当前应用失败（可能缺少写入权限）：{e}"))?;
    }

    if let Err(e) = move_or_ditto_bundle(&staging_app, target_app) {
        if had_existing && backup_app.exists() {
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
    if t.is_dir() {
        t
    } else {
        PathBuf::from("/tmp")
    }
}

/// 阶段 1（下载）：流式下载最新 `.dmg` 到独立临时目录（进度经回调上报，lib.rs 负责 emit），完成后做 SHA-256 校验，返回临时工作目录路径。
pub async fn download_to_staging(
    on_progress: ProgressCallback<'_>,
    dmg_url: Option<String>,
    dmg_sha256: Option<String>,
) -> Result<String, String> {
    let exe = std::env::current_exe().map_err(|e| format!("无法定位当前程序路径：{e}"))?;
    let _target_app = resolve_target_app_bundle(&exe)?;

    let (resolved_url, resolved_sha) = match dmg_url.filter(|u| !u.trim().is_empty()) {
        Some(u) => (u, dmg_sha256),
        None => {
            let info = check(env!("CARGO_PKG_VERSION")).await?;
            let u = info
                .dmg_url
                .ok_or_else(|| "最新 Release 中未找到 .dmg 安装包".to_string())?;
            (u, info.dmg_sha256)
        }
    };

    validate_dmg_url(&resolved_url)?;

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
    let proxy_port = active_local_proxy_port();
    if let Err(e) =
        stream_dmg_to_file(&resolved_url, env!("CARGO_PKG_VERSION"), &dmg_path, proxy_port, on_progress)
            .await
    {
        let _ = std::fs::remove_dir_all(&work_dir);
        return Err(format!("下载更新包失败：{e}"));
    }

    let dmg_for_verify = dmg_path.clone();
    let sha_for_verify = resolved_sha.clone();
    let verify_res = tauri::async_runtime::spawn_blocking(move || {
        verify_sha256_file(&dmg_for_verify, sha_for_verify.as_deref())
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
    let file_name = work_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    if !file_name.starts_with("clash-assist-update-") || !work_dir.is_dir() {
        return Err("无效的临时更新目录".into());
    }
    let dmg_path = work_dir.join("update.dmg");
    if !dmg_path.is_file() {
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

    #[test]
    fn semver_compare() {
        assert!(is_newer("0.2.1", "0.2.0"));
        assert!(is_newer("v0.10.0", "0.9.9"));
        assert!(is_newer("1.0.0", "0.99.99"));
        assert!(!is_newer("0.2.0", "0.2.0"));
        assert!(!is_newer("0.1.9", "0.2.0"));
        assert!(is_newer("0.2.0", "0.2.0-beta.1"));
        assert!(!is_newer("0.2.0-beta.2", "0.2.0"));
        assert!(!is_newer("garbage", "0.2.0"));
        assert!(!is_newer("0.2", "0.1.0"));
    }

    #[test]
    fn only_project_urls_open() {
        assert!(open_release_page("https://evil.example/").is_err());
        assert!(open_release_page("https://github.com/other/repo/releases").is_err());
    }

    #[test]
    fn validate_and_parse_release_dmg_asset() {
        assert!(validate_dmg_url(
            "https://github.com/JTee77/clash-assist/releases/download/v0.2.4/Clash.Assist_0.2.4_aarch64.dmg"
        )
        .is_ok());
        assert!(validate_dmg_url("https://evil.example/Clash.Assist.dmg").is_err());
        assert!(validate_dmg_url(
            "https://github.com/JTee77/clash-assist/releases/download/v0.2.4/script.sh"
        )
        .is_err());

        let sample = serde_json::json!({
            "tag_name": "v0.2.4",
            "assets": [
                {
                    "name": "Clash.Assist_0.2.4_aarch64.dmg",
                    "browser_download_url": "https://github.com/JTee77/clash-assist/releases/download/v0.2.4/Clash.Assist_0.2.4_aarch64.dmg",
                    "digest": "sha256:251112b7cd87706978a3ad57b15b3d4191dfacc65fefa8d9b5c36da01482dbdc"
                }
            ]
        });
        let (dmg, sha) = parse_release_dmg_asset(&sample);
        assert_eq!(
            dmg.as_deref(),
            Some("https://github.com/JTee77/clash-assist/releases/download/v0.2.4/Clash.Assist_0.2.4_aarch64.dmg")
        );
        assert_eq!(
            sha.as_deref(),
            Some("sha256:251112b7cd87706978a3ad57b15b3d4191dfacc65fefa8d9b5c36da01482dbdc")
        );
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
        let dev = Path::new("/Users/test/Projects/clash-assist/src-tauri/target/debug/clash-assist");
        assert!(bundle_path_from_exe(dev).is_none());
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
        std::fs::write(new_app.join("Contents/Info.plist"), plist).unwrap();
        let c_src = base.join("main.c");
        std::fs::write(&c_src, "int main(void) { return 0; }\n").unwrap();
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
        let create_dmg = Command::new("/usr/bin/hdiutil")
            .args(["create", "-volname", "Clash Assist", "-srcfolder"])
            .arg(&src_dir)
            .args(["-ov", "-format", "UDZO"])
            .arg(&dmg_path)
            .output()
            .unwrap();
        assert!(create_dmg.status.success(), "hdiutil create failed");

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
        std::fs::write(target_app.join("Contents/old.txt"), "old-0.2.4").unwrap();

        let work_dir = base.join("work");
        std::fs::create_dir_all(&work_dir).unwrap();

        apply_dmg_to_app_bundle(&dmg_path, &target_app, &work_dir).expect("apply_dmg should succeed");

        assert!(!target_app.join("Contents/old.txt").exists());
        let new_plist = std::fs::read_to_string(target_app.join("Contents/Info.plist")).unwrap();
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

    /// 无 GUI 进度管线测试：本地 HTTP 服务器分块供流，断言
    /// 文件完整落盘、百分比严格递增、收尾补发 100%。
    #[test]
    fn streaming_reports_monotonic_percent() {
        use std::io::{Read, Write as IoWrite};

        const TOTAL: usize = 1_000_000;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            let mut buf = [0u8; 4096];
            let _ = sock.read(&mut buf); // 丢弃请求行与头（单请求连接）
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {TOTAL}\r\nContent-Type: application/octet-stream\r\nConnection: close\r\n\r\n"
            );
            sock.write_all(head.as_bytes()).unwrap();
            for _ in 0..10 {
                sock.write_all(&vec![b'a'; TOTAL / 10]).unwrap();
                sock.flush().unwrap();
                std::thread::sleep(Duration::from_millis(30));
            }
        });

        let dir = std::env::temp_dir().join(format!("ca-stream-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let dest = dir.join("t.dmg");

        let seen: std::sync::Arc<std::sync::Mutex<Vec<u8>>> = Default::default();
        let sink = seen.clone();
        let on_progress = move |p: DownloadProgress| {
            if let Some(pct) = p.percent {
                sink.lock().unwrap().push(pct);
            }
        };

        let url = format!("http://127.0.0.1:{port}/t.dmg");
        tauri::async_runtime::block_on(stream_dmg_to_file(
            &url,
            "test",
            &dest,
            None, // 不走本地代理探测，直连本地测试服务器
            &on_progress,
        ))
        .unwrap();
        server.join().unwrap();

        assert_eq!(std::fs::metadata(&dest).unwrap().len(), TOTAL as u64);
        let pcts = seen.lock().unwrap().clone();
        assert!(!pcts.is_empty(), "至少应有一次进度回调");
        assert_eq!(*pcts.last().unwrap(), 100, "收尾必须补发 100%");
        assert!(
            pcts.windows(2).all(|w| w[0] < w[1]),
            "百分比必须严格递增: {pcts:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
