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
/// GitHub Release API JSON 上限（正常几 KB～几十 KB；给足余量防异常膨胀吃内存）。
const RELEASE_JSON_MAX_BYTES: usize = 4 * 1024 * 1024;
/// 安装包流式下载硬上限（当前 aarch64 DMG 约数十 MB；超过视为异常 / 攻击）。
const DMG_MAX_BYTES: u64 = 512 * 1024 * 1024;

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

const REPO_OWNER: &str = "JTee77";
const REPO_NAME: &str = "clash-assist";

/// 原始字符串层面的预检：拒绝 `..`、反斜杠、控制字符与百分号编码的 `.` `/` `\\`。
/// 必须在 URL 解析之前做——解析器会把 `%2e%2e` 之类的编码形式规范化掉，事后再查就晚了。
fn reject_suspicious_raw(raw: &str) -> Result<(), String> {
    let lower = raw.to_ascii_lowercase();
    if lower.contains("..")
        || lower.contains('\\')
        || lower.contains("%2e")
        || lower.contains("%2f")
        || lower.contains("%5c")
        || lower.contains("%00")
        || raw.chars().any(|c| c.is_control() || c.is_whitespace())
    {
        return Err("地址包含不允许的字符或路径穿越片段".into());
    }
    Ok(())
}

/// 解析为 `https://github.com/...`：协议、主机、端口、用户信息、查询串、片段全部精确限定。
fn parse_pinned_github_url(raw: &str) -> Result<url::Url, String> {
    reject_suspicious_raw(raw)?;
    let u = url::Url::parse(raw).map_err(|_| "地址无法解析".to_string())?;
    if u.scheme() != "https"
        || u.host_str() != Some("github.com")
        || u.port().is_some()
        || !u.username().is_empty()
        || u.password().is_some()
        || u.query().is_some()
        || u.fragment().is_some()
    {
        return Err("只允许 https://github.com 下的本项目地址".into());
    }
    Ok(u)
}

/// 路径段（tag / 文件名）只允许 `[A-Za-z0-9._-]`，且不能是 `.` / `..` 或以 `.` 开头。
fn is_safe_segment(seg: &str) -> bool {
    !seg.is_empty()
        && !seg.starts_with('.')
        && seg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
}

/// 校验 DMG 下载地址：解析后精确比对，必须是
/// `https://github.com/JTee77/clash-assist/releases/download/<tag>/<文件名>.dmg`
/// （owner / repo 逐段精确相等，不接受前缀相似、编码绕过、`..` 或其他主机）。
pub fn validate_dmg_url(url: &str) -> Result<(), String> {
    const ERR: &str = "只允许下载本项目 GitHub Releases 下的 .dmg 安装包";
    let u = parse_pinned_github_url(url.trim()).map_err(|_| ERR.to_string())?;
    let segs: Vec<&str> = u.path().split('/').collect();
    // path 以 "/" 开头，split 的第一段是空串。
    match segs.as_slice() {
        ["", owner, repo, "releases", "download", tag, file]
            if *owner == REPO_OWNER
                && *repo == REPO_NAME
                && is_safe_segment(tag)
                && is_safe_segment(file)
                && file.to_ascii_lowercase().ends_with(".dmg") =>
        {
            Ok(())
        }
        _ => Err(ERR.into()),
    }
}

/// 校验 release 页面地址：`https://github.com/JTee77/clash-assist/releases`、
/// `.../releases/latest` 或 `.../releases/tag/<tag>`，其余一律拒绝。
pub fn validate_release_page_url(url: &str) -> Result<(), String> {
    const ERR: &str = "只允许打开本项目的 release 页面";
    let u = parse_pinned_github_url(url.trim()).map_err(|_| ERR.to_string())?;
    let path = u.path().trim_end_matches('/');
    let segs: Vec<&str> = path.split('/').collect();
    let ok = match segs.as_slice() {
        ["", owner, repo, "releases"] => *owner == REPO_OWNER && *repo == REPO_NAME,
        ["", owner, repo, "releases", "latest"] => *owner == REPO_OWNER && *repo == REPO_NAME,
        ["", owner, repo, "releases", "tag", tag] => {
            *owner == REPO_OWNER && *repo == REPO_NAME && is_safe_segment(tag)
        }
        _ => false,
    };
    if ok {
        Ok(())
    } else {
        Err(ERR.into())
    }
}

/// 规范化 GitHub asset `digest`：只接受 `sha256:<64 位十六进制>`（或裸 64 位十六进制），返回小写十六进制。
pub fn normalize_sha256_digest(raw: &str) -> Option<String> {
    let raw = raw.trim();
    let hex = raw
        .strip_prefix("sha256:")
        .or_else(|| raw.strip_prefix("SHA256:"))
        .unwrap_or(raw)
        .trim()
        .to_ascii_lowercase();
    if hex.len() == 64 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
        Some(hex)
    } else {
        None
    }
}

/// 从 GitHub Release JSON 的 `assets` 数组中提取 `.dmg` 下载直链与 `sha256:...` 摘要。
/// 直链必须指向本项目、且 tag 段与 release `tag_name` 一致、文件名与 asset `name` 一致；
/// 摘要格式不合法时视为缺失（下载阶段会因此拒绝，见 [`resolve_download_target`]）。
pub fn parse_release_dmg_asset(v: &serde_json::Value) -> (Option<String>, Option<String>) {
    let Some(assets) = v["assets"].as_array() else {
        return (None, None);
    };
    let tag = v["tag_name"].as_str().unwrap_or("");
    for asset in assets {
        let name = asset["name"].as_str().unwrap_or("");
        if !name.to_ascii_lowercase().ends_with(".dmg") {
            continue;
        }
        let dl = asset["browser_download_url"].as_str().unwrap_or("");
        if validate_dmg_url(dl).is_err() {
            continue;
        }
        if !tag.is_empty() {
            let expected_suffix = format!("/releases/download/{tag}/{name}");
            if !dl.ends_with(&expected_suffix) {
                continue;
            }
        }
        let digest = asset["digest"]
            .as_str()
            .and_then(normalize_sha256_digest)
            .map(|hex| format!("sha256:{hex}"));
        return (Some(dl.to_string()), digest);
    }
    (None, None)
}

/// 下载阶段的目标：由 Rust 侧自己从 release 元数据解析得到，不接受前端传入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadTarget {
    pub url: String,
    /// 小写 64 位十六进制
    pub sha256: String,
}

/// 从一次 `check` 的结果确定下载目标。fail closed：没有更新、没有 .dmg、没有合法摘要都拒绝。
pub fn resolve_download_target(info: &UpdateInfo) -> Result<DownloadTarget, String> {
    if !info.update_available {
        return Err("当前已是最新版本，无需下载".into());
    }
    let url = info
        .dmg_url
        .clone()
        .ok_or_else(|| "最新 Release 中未找到 .dmg 安装包".to_string())?;
    validate_dmg_url(&url)?;
    let sha256 = info
        .dmg_sha256
        .as_deref()
        .and_then(normalize_sha256_digest)
        .ok_or_else(|| {
            "最新 Release 未提供安装包 SHA-256 摘要，为安全起见已停止一键更新，请前往下载页手动更新"
                .to_string()
        })?;
    Ok(DownloadTarget { url, sha256 })
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
        .user_agent(format!("clash-assist/{current}"))
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            // Release 资产会从 github.com 302 到 *.githubusercontent.com；只跟随 https 且仍在 GitHub 域内的跳转。
            if attempt.previous().len() >= 10 {
                attempt.error("too many redirects")
            } else if is_allowed_github_redirect(attempt.url()) {
                attempt.follow()
            } else {
                attempt.error("redirect target not allowed")
            }
        }));
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

/// 更新链路允许的跳转目标：https + github.com / api.github.com / *.githubusercontent.com，无自定义端口。
fn is_allowed_github_redirect(u: &url::Url) -> bool {
    if u.scheme() != "https" || u.port().is_some() {
        return false;
    }
    match u.host_str() {
        Some(h) => {
            let h = h.to_ascii_lowercase();
            h == "github.com" || h == "api.github.com" || h.ends_with(".githubusercontent.com")
        }
        None => false,
    }
}

async fn get_bytes_with_proxy_fallback(
    url: &str,
    current: &str,
    timeout_ms: u64,
    connect_ms: u64,
    accept_header: Option<&str>,
) -> Result<Vec<u8>, String> {
    get_bytes_via(
        url,
        current,
        timeout_ms,
        connect_ms,
        accept_header,
        active_local_proxy_port(),
    )
    .await
}

/// 同上，代理端口显式注入（单测传 None，避免本机开着代理时结果受环境影响）。
async fn get_bytes_via(
    url: &str,
    current: &str,
    timeout_ms: u64,
    connect_ms: u64,
    accept_header: Option<&str>,
    proxy_port: Option<u16>,
) -> Result<Vec<u8>, String> {
    if let Some(port) = proxy_port {
        if let Ok(client) = build_http_client(current, Some(timeout_ms), connect_ms, Some(port)) {
            let mut req = client.get(url);
            if let Some(acc) = accept_header {
                req = req.header("Accept", acc);
            }
            if let Ok(resp) = req.send().await {
                if resp.status().is_success() {
                    if let Ok(bytes) =
                        crate::http_body::read_body_capped(resp, RELEASE_JSON_MAX_BYTES).await
                    {
                        return Ok(bytes);
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
    crate::http_body::read_body_capped(resp, RELEASE_JSON_MAX_BYTES)
        .await
        .map_err(|e| format!("读取 GitHub 响应失败：{e}"))
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
    stream_dmg_to_file_capped(url, current, dest, proxy_port, on_progress, DMG_MAX_BYTES).await
}

/// 同 `stream_dmg_to_file`，上限可注入（单测用小上限）。超限是确定性错误：不再回落重试。
async fn stream_dmg_to_file_capped(
    url: &str,
    current: &str,
    dest: &Path,
    proxy_port: Option<u16>,
    on_progress: ProgressCallback<'_>,
    max_bytes: u64,
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
        if let Some(t) = total {
            if t > max_bytes {
                return Err(format!(
                    "安装包过大（Content-Length {t} > {max_bytes} 上限），已拒绝下载"
                ));
            }
        }
        let mut file = match crate::safe_fs::create_truncate(dest) {
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
            if downloaded > max_bytes {
                drop(file);
                let _ = std::fs::remove_file(dest);
                return Err(format!(
                    "安装包过大（已下载 {downloaded} > {max_bytes} 上限），已中止"
                ));
            }

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
        .filter(|u| validate_release_page_url(u).is_ok())
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
/// 用 `open` crate 交给系统：macOS 为 `/usr/bin/open -- <url>`、Windows 为 ShellExecuteExW，
/// 均不经 shell 解析（旧实现在 Windows 上用 `cmd /C start`，URL 中的 `&` 等会被 cmd 解释）。
pub fn open_release_page(url: &str) -> Result<(), String> {
    let url = url.trim();
    validate_release_page_url(url)?;
    open::that_detached(url).map_err(|e| format!("打开浏览器失败：{e}"))
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

/// 进程内计算并校验下载的 DMG 文件 SHA-256。fail closed：未提供摘要或摘要格式不合法时一律报错。
pub fn verify_sha256_file(path: &Path, expected_digest: Option<&str>) -> Result<(), String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;

    let Some(raw) = expected_digest.map(str::trim).filter(|s| !s.is_empty()) else {
        return Err("缺少安装包 SHA-256 摘要，拒绝安装未校验的更新包".into());
    };
    let expected =
        normalize_sha256_digest(raw).ok_or_else(|| format!("无效的 SHA-256 摘要格式：{raw}"))?;
    let mut file = crate::safe_fs::open_read(path).map_err(|e| format!("打开安装包失败：{e}"))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = file
            .read(&mut buf)
            .map_err(|e| format!("读取安装包失败：{e}"))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let actual: String = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
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
    let DownloadTarget {
        url: resolved_url,
        sha256: resolved_sha,
    } = resolve_download_target(&info)?;

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
    if let Err(e) = stream_dmg_to_file(
        &resolved_url,
        env!("CARGO_PKG_VERSION"),
        &dmg_path,
        proxy_port,
        on_progress,
    )
    .await
    {
        let _ = std::fs::remove_dir_all(&work_dir);
        return Err(format!("下载更新包失败：{e}"));
    }

    let dmg_for_verify = dmg_path.clone();
    let verify_res = tauri::async_runtime::spawn_blocking(move || {
        verify_sha256_file(&dmg_for_verify, Some(&resolved_sha))
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

    const GOOD_DMG: &str =
        "https://github.com/JTee77/clash-assist/releases/download/v0.2.9/Clash.Assist_0.2.9_aarch64.dmg";
    const GOOD_SHA: &str =
        "sha256:251112b7cd87706978a3ad57b15b3d4191dfacc65fefa8d9b5c36da01482dbdc";

    #[test]
    fn dmg_url_rejects_traversal_and_encoded_variants() {
        assert!(validate_dmg_url(GOOD_DMG).is_ok());
        for bad in [
            // 路径穿越：旧前缀匹配会放行，解析后实际落到其他仓库
            "https://github.com/JTee77/clash-assist/releases/download/../../../../evil/repo/releases/download/v1/x.dmg",
            "https://github.com/JTee77/clash-assist/releases/download/v0.2.9/../../../../../evil/x.dmg",
            // 编码形式的 `.` / `/` / `\`
            "https://github.com/JTee77/clash-assist/releases/download/%2e%2e/%2e%2e/evil/x.dmg",
            "https://github.com/JTee77/clash-assist/releases/download/%2E%2E%2Fevil/x.dmg",
            "https://github.com/JTee77/clash-assist/releases/download/v0.2.9%2f..%2fx/x.dmg",
            "https://github.com/JTee77/clash-assist/releases/download/v0.2.9%5cx/x.dmg",
            "https://github.com/JTee77/clash-assist/releases/download/v0.2.9\\..\\x.dmg",
            "https://github.com/JTee77/clash-assist/releases/download/.hidden/x.dmg",
        ] {
            assert!(validate_dmg_url(bad).is_err(), "should reject: {bad}");
        }
    }

    #[test]
    fn dmg_url_pins_scheme_host_owner_repo_and_shape() {
        for bad in [
            // 其他 owner / repo（含前缀相似）
            "https://github.com/evil/clash-assist/releases/download/v0.2.9/x.dmg",
            "https://github.com/JTee77/clash-assist-evil/releases/download/v0.2.9/x.dmg",
            "https://github.com/jtee77/clash-assist/releases/download/v0.2.9/x.dmg",
            // http / 其他主机 / 端口 / 用户信息
            "http://github.com/JTee77/clash-assist/releases/download/v0.2.9/x.dmg",
            "https://evil.example/JTee77/clash-assist/releases/download/v0.2.9/x.dmg",
            "https://github.com.evil.example/JTee77/clash-assist/releases/download/v0.2.9/x.dmg",
            "https://github.com:8443/JTee77/clash-assist/releases/download/v0.2.9/x.dmg",
            "https://user@github.com/JTee77/clash-assist/releases/download/v0.2.9/x.dmg",
            "https://github.com@evil.example/JTee77/clash-assist/releases/download/v0.2.9/x.dmg",
            // 查询串 / 片段 / 额外段 / 非 dmg
            "https://github.com/JTee77/clash-assist/releases/download/v0.2.9/x.dmg?x=1",
            "https://github.com/JTee77/clash-assist/releases/download/v0.2.9/x.dmg#frag",
            "https://github.com/JTee77/clash-assist/releases/download/v0.2.9/extra/x.dmg",
            "https://github.com/JTee77/clash-assist/releases/download/v0.2.9/x.sh",
            "https://github.com/JTee77/clash-assist/archive/v0.2.9/x.dmg",
            "",
            "not a url",
        ] {
            assert!(validate_dmg_url(bad).is_err(), "should reject: {bad}");
        }
    }

    #[test]
    fn release_page_url_validation() {
        for ok in [
            "https://github.com/JTee77/clash-assist/releases",
            "https://github.com/JTee77/clash-assist/releases/",
            "https://github.com/JTee77/clash-assist/releases/latest",
            "https://github.com/JTee77/clash-assist/releases/tag/v0.2.9",
        ] {
            assert!(validate_release_page_url(ok).is_ok(), "should accept: {ok}");
        }
        for bad in [
            "https://github.com/JTee77/clash-assist/releasesX",
            "https://github.com/JTee77/clash-assist/releases/../../evil/repo",
            "https://github.com/JTee77/clash-assist/releases/%2e%2e/%2e%2e/evil",
            "https://github.com/JTee77/clash-assist/releases/tag/v1&calc.exe",
            "https://github.com/JTee77/clash-assist/releases?x=1",
            "https://github.com/JTee77/clash-assist/releases.evil.example/",
            "http://github.com/JTee77/clash-assist/releases",
            "https://github.com/other/repo/releases",
            "file:///etc/passwd",
            "javascript:alert(1)",
        ] {
            assert!(
                validate_release_page_url(bad).is_err(),
                "should reject: {bad}"
            );
            assert!(open_release_page(bad).is_err(), "open should reject: {bad}");
        }
    }

    #[test]
    fn release_asset_must_match_tag_and_name() {
        let mismatched_tag = serde_json::json!({
            "tag_name": "v0.2.9",
            "assets": [{
                "name": "Clash.Assist_0.2.9_aarch64.dmg",
                "browser_download_url": "https://github.com/JTee77/clash-assist/releases/download/v0.1.0/Clash.Assist_0.2.9_aarch64.dmg",
                "digest": GOOD_SHA
            }]
        });
        assert_eq!(parse_release_dmg_asset(&mismatched_tag), (None, None));

        // 只有 API `url` 字段（非 browser_download_url）不再被当作下载直链
        let api_url_only = serde_json::json!({
            "tag_name": "v0.2.9",
            "assets": [{ "name": "Clash.Assist_0.2.9_aarch64.dmg", "url": GOOD_DMG, "digest": GOOD_SHA }]
        });
        assert_eq!(parse_release_dmg_asset(&api_url_only), (None, None));

        // 摘要格式不合法视为缺失
        let bad_digest = serde_json::json!({
            "tag_name": "v0.2.9",
            "assets": [{ "name": "Clash.Assist_0.2.9_aarch64.dmg", "browser_download_url": GOOD_DMG, "digest": "md5:abc" }]
        });
        assert_eq!(
            parse_release_dmg_asset(&bad_digest),
            (Some(GOOD_DMG.into()), None)
        );
    }

    fn info(update_available: bool, dmg_url: Option<&str>, sha: Option<&str>) -> UpdateInfo {
        UpdateInfo {
            current: "0.2.8".into(),
            latest: "0.2.9".into(),
            update_available,
            url: format!("{RELEASES_PAGE}/tag/v0.2.9"),
            fake: false,
            dmg_url: dmg_url.map(str::to_string),
            dmg_sha256: sha.map(str::to_string),
        }
    }

    #[test]
    fn download_target_fails_closed_without_digest() {
        let ok = resolve_download_target(&info(true, Some(GOOD_DMG), Some(GOOD_SHA))).unwrap();
        assert_eq!(ok.url, GOOD_DMG);
        assert_eq!(ok.sha256, GOOD_SHA.trim_start_matches("sha256:"));

        let no_digest = resolve_download_target(&info(true, Some(GOOD_DMG), None)).unwrap_err();
        assert!(no_digest.contains("SHA-256"), "{no_digest}");
        assert!(resolve_download_target(&info(true, Some(GOOD_DMG), Some(""))).is_err());
        assert!(resolve_download_target(&info(true, Some(GOOD_DMG), Some("sha256:xyz"))).is_err());
        assert!(resolve_download_target(&info(true, None, Some(GOOD_SHA))).is_err());
        assert!(resolve_download_target(&info(false, Some(GOOD_DMG), Some(GOOD_SHA))).is_err());
        assert!(resolve_download_target(&info(
            true,
            Some("https://evil.example/x.dmg"),
            Some(GOOD_SHA)
        ))
        .is_err());
    }

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
    fn redirect_targets_limited_to_github_https() {
        let ok = |s: &str| is_allowed_github_redirect(&url::Url::parse(s).unwrap());
        assert!(ok("https://release-assets.githubusercontent.com/x"));
        assert!(ok("https://objects.githubusercontent.com/x"));
        assert!(ok("https://github.com/x"));
        assert!(!ok("http://objects.githubusercontent.com/x"));
        assert!(!ok("https://evil.example/x"));
        assert!(!ok("https://githubusercontent.com.evil.example/x"));
        assert!(!ok("https://objects.githubusercontent.com:444/x"));
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

        assert_eq!(
            crate::safe_fs::open_read(&dest)
                .unwrap()
                .metadata()
                .unwrap()
                .len(),
            TOTAL as u64
        );
        let pcts = seen.lock().unwrap().clone();
        assert!(!pcts.is_empty(), "至少应有一次进度回调");
        assert_eq!(*pcts.last().unwrap(), 100, "收尾必须补发 100%");
        assert!(
            pcts.windows(2).all(|w| w[0] < w[1]),
            "百分比必须严格递增: {pcts:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 本地夹具：接一个连接，读掉请求头后写 `response` 并关闭。
    fn serve_raw_once(response: Vec<u8>) -> (u16, std::thread::JoinHandle<()>) {
        use std::io::{Read, Write as IoWrite};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let h = std::thread::spawn(move || {
            if let Ok((mut s, _)) = listener.accept() {
                let _ = s.set_read_timeout(Some(Duration::from_millis(1000)));
                let mut buf = [0u8; 4096];
                let mut got = Vec::new();
                while !got.windows(4).any(|w| w == b"\r\n\r\n") {
                    match s.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => got.extend_from_slice(&buf[..n]),
                    }
                }
                let _ = s.write_all(&response);
            }
        });
        (port, h)
    }

    fn chunked_body(chunks: usize, chunk_len: usize) -> Vec<u8> {
        let mut r =
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n".to_vec();
        for _ in 0..chunks {
            r.extend_from_slice(format!("{chunk_len:x}\r\n").as_bytes());
            r.extend(std::iter::repeat_n(b'a', chunk_len));
            r.extend_from_slice(b"\r\n");
        }
        r.extend_from_slice(b"0\r\n\r\n");
        r
    }

    #[test]
    fn release_json_read_is_capped() {
        // 声明 Content-Length 超过 RELEASE_JSON_MAX_BYTES：不读 body 直接拒绝。
        let head = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{{}}",
            RELEASE_JSON_MAX_BYTES + 1
        );
        let (port, h) = serve_raw_once(head.into_bytes());
        let url = format!("http://127.0.0.1:{port}/latest");
        let r = tauri::async_runtime::block_on(get_bytes_via(&url, "test", 3000, 1000, None, None));
        let _ = h.join();
        let err = r.expect_err("oversized release JSON must be rejected");
        assert!(err.contains("too large"), "{err}");

        // 正常大小的 JSON 照常返回。
        let ok = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}".to_vec();
        let (port, h) = serve_raw_once(ok);
        let url = format!("http://127.0.0.1:{port}/latest");
        let r = tauri::async_runtime::block_on(get_bytes_via(&url, "test", 3000, 1000, None, None));
        let _ = h.join();
        assert_eq!(r.unwrap(), b"{}");
    }

    fn dmg_test_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("ca-dmg-cap-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn dmg_download_rejects_declared_oversize() {
        let resp =
            b"HTTP/1.1 200 OK\r\nContent-Length: 999999\r\nConnection: close\r\n\r\nabc".to_vec();
        let (port, h) = serve_raw_once(resp);
        let dir = dmg_test_dir("cl");
        let dest = dir.join("t.dmg");
        let url = format!("http://127.0.0.1:{port}/t.dmg");
        let r = tauri::async_runtime::block_on(stream_dmg_to_file_capped(
            &url,
            "test",
            &dest,
            None,
            &|_| {},
            1024,
        ));
        let _ = h.join();
        let err = r.expect_err("declared oversize DMG must be rejected");
        assert!(err.contains("过大"), "{err}");
        assert!(!crate::safe_fs::exists(&dest), "nothing should be written");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dmg_download_aborts_when_stream_exceeds_cap() {
        // 无 Content-Length 的 chunked 流，总 8 KiB > 2 KiB 上限：边下边判、删掉半截文件。
        let (port, h) = serve_raw_once(chunked_body(8, 1024));
        let dir = dmg_test_dir("chunked");
        let dest = dir.join("t.dmg");
        let url = format!("http://127.0.0.1:{port}/t.dmg");
        let r = tauri::async_runtime::block_on(stream_dmg_to_file_capped(
            &url,
            "test",
            &dest,
            None,
            &|_| {},
            2048,
        ));
        let _ = h.join();
        let err = r.expect_err("oversized DMG stream must abort");
        assert!(err.contains("过大"), "{err}");
        assert!(
            !crate::safe_fs::exists(&dest),
            "partial file must be removed"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
