//! 更新检查与应用内自更新（windows-plan r4 §2.4，M5）。
//!
//! - `mod.rs`（通用）：版本比较（`semver`，S-11）、release 与资产校验（V8、S-5 ②、C-3）、
//!   流式下载、`sha256_of_reader`（M-4）、代理端口探测（S-14 记日志）、测试专用开关（M-7）。
//! - `macos.rs`：DMG 安装链（非 Windows 编译）。
//! - `windows.rs`：NSIS 安装链（`install_kind` / `prepare_update_dir` / `download_and_verify` /
//!   `launch_installer`，仅 Windows 编译）。
//! - `install.rs`：Windows 安装链中的纯逻辑（安装类型判定、PE 版本比对、启动后观察），
//!   Windows 与各平台单测都编译。
//! - `pending.rs`：「更新待完成」标记与启动对账（Windows 使用）。
//! - `e2e.rs`：`updater-e2e` feature 专用开关；非 e2e 构建中这些环境变量与标记都不存在。
//!
//! 入口（lib.rs 的命令）：`check` → `download_to_staging` →（Mac）`apply_staged_update` →
//! `relaunch_updated_app`（Mac：拉起新 .app；Windows：启动安装程序 `/S /UPDATE /R` 后退出）。

use serde::Serialize;
use std::path::Path;
use std::time::{Duration, Instant};

pub(crate) mod e2e;
#[cfg(any(windows, test))]
pub(crate) mod install;
pub mod pending;

#[cfg(not(windows))]
mod macos;
#[cfg(windows)]
mod windows;

pub const LATEST_RELEASE_API: &str =
    "https://api.github.com/repos/JTee77/clash-assist/releases/latest";
pub const RELEASES_PAGE: &str = "https://github.com/JTee77/clash-assist/releases";
pub const DOWNLOAD_PREFIX: &str = "https://github.com/JTee77/clash-assist/releases/download/";
/// 测试用假版本号（跳过网络）。M-7：只在 debug 构建与 `updater-e2e` feature 中编译，
/// release 构建的 exe 里不含这个字符串（`scan-exe-markers.ps1` 对 release 构建发现即失败）。
#[cfg(any(debug_assertions, feature = "updater-e2e"))]
pub const FAKE_LATEST_ENV: &str = "CLASH_ASSIST_FAKE_LATEST";
const TIMEOUT_MS: u64 = 5000;
/// 下载进度事件名（前端 useUpdateFlow 监听）。
pub const DOWNLOAD_PROGRESS_EVENT: &str = "update-download-progress";
/// 单块空闲超时：30 秒没有新数据视为连接停滞。替代旧的全局 120s 总超时——
/// 大安装包在慢网络下会被总超时误杀；流式下载有进度事件，停滞对用户可见。
const DOWNLOAD_IDLE_TIMEOUT_MS: u64 = 30_000;
/// GitHub Release API JSON 上限（正常几 KB～几十 KB；给足余量防异常膨胀吃内存）。
const RELEASE_JSON_MAX_BYTES: usize = 4 * 1024 * 1024;

/// 扫描器的专用正向对照（S-2，Ellie PR58）：`scan-exe-markers.ps1` 在每个 exe 里都必须找到它，
/// 否则判定扫描器失效。不再依赖业务代码（`LATEST_RELEASE_API`）是否被编译器保留。
/// `#[used]` + 启动时经 `black_box` 取一次地址（[`keep_build_markers`]），防止被链接器丢掉。
#[used]
static SCAN_POSITIVE_CONTROL: [u8; 34] = *b"CLASH_ASSIST_SCAN_POSITIVE_CONTROL";

/// 在 `run()` 开头调用一次：让扫描用的标记常量（正向对照；e2e 构建另有 e2e 标记）留在 exe 里（S-R4）。
pub fn keep_build_markers() {
    std::hint::black_box(SCAN_POSITIVE_CONTROL.as_ptr());
    e2e::keep_marker();
}

/// 下载进度（camelCase 序列化给前端）。
#[derive(Debug, Clone, Serialize)]
pub struct DownloadProgress {
    pub downloaded: u64,
    pub total: Option<u64>,
    pub percent: Option<u8>,
}

/// 安装形态（windows-plan §2.4 S-10）。Mac 为 `dmg`；Windows 由 `install_kind()` 判定。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
#[allow(dead_code)] // Dmg 只在非 Windows、Nsis / Portable 只在 Windows 上构造
pub enum InstallKind {
    Dmg,
    Nsis,
    Portable,
}

/// 「上次更新」的两端版本（pending 标记对账结果，见 `pending.rs`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VersionPair {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Serialize, Clone)]
pub struct UpdateInfo {
    pub current: String,
    pub latest: String,
    pub update_available: bool,
    pub url: String,
    /// true = 来自 CLASH_ASSIST_FAKE_LATEST（未访问网络；只在测试构建中可能为 true）
    pub fake: bool,
    /// GitHub Release 中的 macOS .dmg 下载直链（仅 Mac，且资产通过全部校验时）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dmg_url: Option<String>,
    /// 上述 .dmg 的 SHA-256 摘要（形如 "sha256:..."）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dmg_sha256: Option<String>,
    /// M5：安装形态（Rust 判定）。
    pub install_kind: InstallKind,
    /// M5：能否一键更新（Rust 判定，前端以此为准）。便携版、release 或资产未通过校验时为 false。
    pub one_click: bool,
    /// 上次更新未完成（当前版本仍是 `from`）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pending: Option<VersionPair>,
    /// 上次更新已完成（当前版本即 `to`），前端提示「已更新」一次。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated: Option<VersionPair>,
    /// 通过全部校验的资产（不发给前端；下载阶段只用它，不接受前端传入）。
    #[serde(skip)]
    pub asset: Option<ReleaseAsset>,
}

/// 「v1.2.3」/「1.2.3-beta.1」→ semver（S-11）。两段号、四段号、前导零等一律非法。
pub fn parse_version(s: &str) -> Option<semver::Version> {
    let s = s.trim();
    let s = s.strip_prefix(['v', 'V']).unwrap_or(s);
    semver::Version::parse(s).ok()
}

/// latest 是否严格新于 current（semver 优先级：预发布 < 正式版，`beta.1` < `beta.2`，忽略构建元数据）。
/// 任一方不是合法版本号时返回 false（fail closed）。
pub fn is_newer(latest: &str, current: &str) -> bool {
    match (parse_version(latest), parse_version(current)) {
        (Some(l), Some(c)) => l.cmp_precedence(&c) == std::cmp::Ordering::Greater,
        _ => false,
    }
}

/// 两个版本号是否表示同一版本（semver 优先级相等）；任一非法则按去掉 `v` 后的字符串比较。
#[cfg_attr(not(windows), allow(dead_code))] // pending 对账（Windows）使用
pub fn same_version(a: &str, b: &str) -> bool {
    match (parse_version(a), parse_version(b)) {
        (Some(x), Some(y)) => x.cmp_precedence(&y) == std::cmp::Ordering::Equal,
        _ => normalize(a) == normalize(b),
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

/// 校验资产下载地址（V8）：解析后必须**逐字节等于** `DOWNLOAD_PREFIX/{tag}/{expected_name}`
/// （owner / repo / tag / 文件名精确相等，不接受前缀相似、编码绕过、`..`、查询串或其他主机）。
/// `updater-e2e` 构建另外接受同一本地源下的对应地址（见 `e2e.rs`）。
pub fn validate_asset_url(url: &str, tag: &str, expected_name: &str) -> Result<(), String> {
    const ERR: &str = "只允许下载本项目 GitHub Releases 下与版本号对应的安装包";
    if !is_safe_segment(tag) || !is_safe_segment(expected_name) {
        return Err(ERR.into());
    }
    if e2e::local_asset_url_ok(url, tag, expected_name) {
        return Ok(());
    }
    let u = parse_pinned_github_url(url).map_err(|_| ERR.to_string())?;
    let want = format!("{DOWNLOAD_PREFIX}{tag}/{expected_name}");
    if url == want && u.as_str() == want {
        Ok(())
    } else {
        Err(ERR.into())
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

#[cfg_attr(windows, allow(dead_code))] // Mac 的 verify_sha256_file 使用
/// 规范化 SHA-256 摘要：接受 `sha256:<64 位十六进制>`（或裸 64 位十六进制），返回小写十六进制。
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

/// 要找的 release 资产（V8 命名契约）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssetKind {
    /// `Clash.Assist_{v}_aarch64.dmg`
    MacDmgAarch64,
    /// `Clash.Assist_{v}_x64-setup.exe`
    WinSetupX64,
}

impl AssetKind {
    /// 本平台的更新资产（Linux 只用于开发与单测，沿用 Mac 路径）。
    pub fn current() -> Self {
        if cfg!(windows) {
            AssetKind::WinSetupX64
        } else {
            AssetKind::MacDmgAarch64
        }
    }

    pub fn expected_name(self, version: &str) -> String {
        match self {
            AssetKind::MacDmgAarch64 => format!("Clash.Assist_{version}_aarch64.dmg"),
            AssetKind::WinSetupX64 => format!("Clash.Assist_{version}_x64-setup.exe"),
        }
    }

    /// 下载硬上限：DMG 512 MiB（沿用）；Windows 安装包 64 MiB（M-4，当前约 10 MB）。
    pub fn max_bytes(self) -> u64 {
        match self {
            AssetKind::MacDmgAarch64 => 512 * 1024 * 1024,
            AssetKind::WinSetupX64 => 64 * 1024 * 1024,
        }
    }
}

/// 通过全部校验的 release 资产。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseAsset {
    pub kind: AssetKind,
    /// release 的 `tag_name`（`v` + semver）
    pub tag: String,
    /// 去掉 `v` 的版本号
    pub version: String,
    pub name: String,
    pub url: String,
    /// 小写 64 位十六进制
    pub sha256: String,
    /// API 给出的字节数；下载必须恰好这么多（M-4）
    pub size: u64,
}

/// 从 GitHub release JSON 中找出 `kind` 对应的资产，并做全部 release / 资产校验（S-5 ②、C-3、V8）。
/// 任何一项不满足都返回 Err（调用方据此关闭一键更新、只给发布页）：
/// - `tag_name` 是 `v` + 合法 semver；
/// - `draft == false`、`prerelease == false`（字段必须存在且为布尔）；
/// - **`immutable == true`（C-3）：字段缺失、为 `null`、为 `false`、为字符串 `"true"` 一律拒绝**。
///   Windows 与 Mac 都无条件要求（Mac 的取舍见 M5 PR 说明）；不做「字段存在才检查」之类的条件判断；
/// - 资产名逐字节等于 V8 命名（`Clash.Assist_{v}_x64-setup.exe` / `Clash.Assist_{v}_aarch64.dmg`）；
/// - 资产 `state == "uploaded"`；`size` 存在、为正、不超过上限；
/// - `digest` 为 `sha256:` 加 64 位十六进制；
/// - `browser_download_url` 逐字节等于 `DOWNLOAD_PREFIX/{tag}/{name}`。
pub fn parse_release_asset(v: &serde_json::Value, kind: AssetKind) -> Result<ReleaseAsset, String> {
    let tag = v["tag_name"].as_str().ok_or("release 缺少 tag_name")?;
    let version = parse_version(tag)
        .filter(|_| tag.starts_with('v'))
        .ok_or_else(|| format!("release 的 tag 不是 v+版本号：{tag}"))?
        .to_string();
    if tag != format!("v{version}") {
        return Err(format!("release 的 tag 不是规范的 v+版本号：{tag}"));
    }
    if v["draft"].as_bool() != Some(false) {
        return Err("release 不是已发布状态（draft 不为 false）".into());
    }
    if v["prerelease"].as_bool() != Some(false) {
        return Err("release 是预发布或缺少 prerelease 字段".into());
    }
    if v["immutable"].as_bool() != Some(true) {
        return Err("release 不是不可变发布（immutable 不为 true），已关闭一键更新".into());
    }
    let name = kind.expected_name(&version);
    let assets = v["assets"].as_array().ok_or("release 缺少 assets")?;
    let asset = assets
        .iter()
        .find(|a| a["name"].as_str() == Some(name.as_str()))
        .ok_or_else(|| format!("release 中没有 {name}"))?;
    if asset["state"].as_str() != Some("uploaded") {
        return Err(format!("{name} 尚未上传完成（state 不为 uploaded）"));
    }
    let size = asset["size"]
        .as_u64()
        .filter(|s| *s > 0)
        .ok_or_else(|| format!("{name} 缺少 size"))?;
    if size > kind.max_bytes() {
        return Err(format!(
            "{name} 过大（{size} > {} 字节上限）",
            kind.max_bytes()
        ));
    }
    let sha256 = asset["digest"]
        .as_str()
        .and_then(|d| d.strip_prefix("sha256:"))
        .filter(|hex| hex.len() == 64 && hex.chars().all(|c| c.is_ascii_hexdigit()))
        .map(str::to_ascii_lowercase)
        .ok_or_else(|| format!("{name} 缺少合法的 sha256 digest"))?;
    let url = asset["browser_download_url"]
        .as_str()
        .ok_or_else(|| format!("{name} 缺少 browser_download_url"))?;
    validate_asset_url(url, tag, &name)?;
    Ok(ReleaseAsset {
        kind,
        tag: tag.to_string(),
        version,
        name,
        url: url.to_string(),
        sha256,
        size,
    })
}

/// 由一份 release JSON 得到 `UpdateInfo`（纯函数；`check` 与单测共用）。
pub fn info_from_release(
    v: &serde_json::Value,
    current: &str,
    kind: AssetKind,
    install_kind: InstallKind,
) -> Result<UpdateInfo, String> {
    let tag = v["tag_name"].as_str().ok_or("GitHub 响应缺少 tag_name")?;
    let latest = normalize(tag);
    if parse_version(&latest).is_none() {
        return Err(format!("最新 release 的 tag 不是版本号：{tag}"));
    }
    let url = v["html_url"]
        .as_str()
        .filter(|u| validate_release_page_url(u).is_ok())
        .map(str::to_string)
        .unwrap_or_else(|| format!("{RELEASES_PAGE}/tag/{tag}"));
    let update_available = is_newer(&latest, current);
    let asset = if update_available {
        match parse_release_asset(v, kind) {
            Ok(a) => Some(a),
            Err(e) => {
                crate::append_app_log(&format!("[update] 不提供一键更新：{e}"));
                None
            }
        }
    } else {
        None
    };
    let one_click = asset.is_some() && install_kind != InstallKind::Portable;
    let (dmg_url, dmg_sha256) = match &asset {
        Some(a) if a.kind == AssetKind::MacDmgAarch64 => {
            (Some(a.url.clone()), Some(format!("sha256:{}", a.sha256)))
        }
        _ => (None, None),
    };
    Ok(UpdateInfo {
        current: current.to_string(),
        update_available,
        latest,
        url,
        fake: false,
        dmg_url,
        dmg_sha256,
        install_kind,
        one_click,
        pending: None,
        updated: None,
        asset,
    })
}

/// 从一次 `check` 的结果确定下载目标。fail closed：没有更新、便携版、资产未通过校验都拒绝。
/// 下载地址与期望摘要只来自 Rust 侧解析的 release 元数据，不接受前端传入。
pub fn resolve_download_target(info: &UpdateInfo) -> Result<ReleaseAsset, String> {
    if !info.update_available {
        return Err("当前已是最新版本，无需下载".into());
    }
    if info.install_kind == InstallKind::Portable {
        return Err("便携版不支持一键更新，请前往发布页下载".into());
    }
    let asset = info.asset.clone().ok_or_else(|| {
        "最新 Release 未通过安全校验（安装包、SHA-256 摘要或不可变发布），为安全起见已停止一键更新，请前往下载页手动更新"
            .to_string()
    })?;
    validate_asset_url(&asset.url, &asset.tag, &asset.name)?;
    if !is_newer(&asset.version, &info.current) {
        return Err("最新 Release 不比当前版本新，拒绝安装".into());
    }
    Ok(asset)
}

/// 代理端口探测的总时限（M4d）：每个候选的发现（读配置 / 管道探测）最坏约 800 ms，
/// 几个候选串行下来会明显拖慢检查更新与下载。超时即视为未发现代理端口，走原有的直连回退。
const PROXY_PORT_PROBE_BUDGET: Duration = Duration::from_secs(2);

/// 检测本机可能正在监听的代理混合端口（Verge 7897 / FlClash 7890），用于加速 GitHub API 与 Release 下载。
fn active_local_proxy_port() -> Option<u16> {
    use std::net::{SocketAddr, TcpStream};
    probe_local_proxy_port(
        Instant::now() + PROXY_PORT_PROBE_BUDGET,
        |client| crate::mihomo::discover_for_client(client).mixed_port,
        |port| {
            let addr = SocketAddr::from(([127, 0, 0, 1], port));
            TcpStream::connect_timeout(&addr, Duration::from_millis(40)).is_ok()
        },
    )
}

/// `active_local_proxy_port` 的可测试内核：每一步（发现、连接）开始前检查 `deadline`，
/// 过期即停止并返回 `None`（与未发现端口相同）。
fn probe_local_proxy_port(
    deadline: Instant,
    discover: impl Fn(&str) -> u16,
    reachable: impl Fn(u16) -> bool,
) -> Option<u16> {
    let expired = || Instant::now() >= deadline;
    let mut candidates = vec![7897u16, 7890u16];
    if expired() {
        return None;
    }
    let verge = discover("verge");
    if verge > 0 && !candidates.contains(&verge) {
        candidates.insert(0, verge);
    }
    if expired() {
        return None;
    }
    let flclash = discover("flclash");
    if flclash > 0 && !candidates.contains(&flclash) {
        candidates.push(flclash);
    }
    for port in candidates {
        if expired() {
            return None;
        }
        if reachable(port) {
            return Some(port);
        }
    }
    None
}

/// 异步调用方用这个（M4c）：`active_local_proxy_port` 是同步的（读配置、探测管道、短连接），
/// Windows 上的管道探测内部用 `block_on`，在 tokio 工作线程上直接调用会 panic
/// （"Cannot start a runtime from within a runtime"；release 为 `panic = "abort"`，进程以 0xC0000409 退出）。
/// M4d：外层再套 `PROXY_PORT_PROBE_BUDGET` 总时限，超时按未发现端口处理。
async fn active_local_proxy_port_async() -> Option<u16> {
    run_probe_with_budget(PROXY_PORT_PROBE_BUDGET, active_local_proxy_port).await
}

/// 在阻塞线程池上运行 `probe`，最多等待 `budget`；超时（或任务失败）返回 `None`。
/// 超时后阻塞任务不会被强行中止，但同步内核自身也会在截止时间后停止。
async fn run_probe_with_budget<F>(budget: Duration, probe: F) -> Option<u16>
where
    F: FnOnce() -> Option<u16> + Send + 'static,
{
    tokio::time::timeout(budget, tauri::async_runtime::spawn_blocking(probe))
        .await
        .ok()
        .and_then(|r| r.ok())
        .flatten()
}

/// 目标是本机回环地址时不走代理（e2e 本地服务器；单测的本地 HTTP 夹具）。
fn is_loopback_url(url: &str) -> bool {
    url::Url::parse(url)
        .ok()
        .and_then(|u| {
            u.host_str()
                .map(|h| h == "127.0.0.1" || h == "[::1]" || h == "localhost")
        })
        .unwrap_or(false)
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
    let port = if is_loopback_url(url) {
        None
    } else {
        active_local_proxy_port_async().await
    };
    get_bytes_via(url, current, timeout_ms, connect_ms, accept_header, port).await
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

/// 一次下载的约束。
#[derive(Debug, Clone, Copy)]
pub(crate) struct DownloadLimits {
    /// 硬上限：Content-Length 或实际字节数超过即中止。
    pub max_bytes: u64,
    /// API 给出的资产大小（M-4）：Content-Length 不符立即拒绝；实际字节数必须恰好相等。
    pub expected_size: Option<u64>,
}

/// 流式下载到 `open()` 给出的写入端（由 `stream_dmg_to_file` 改名并泛化，M5）。
///
/// - 只在「尚未开始传输」时做代理 → 直连回落；`open()` 只在拿到 2xx 响应后调用**一次**
///   （Windows 用 `create_new` 独占创建，不能调用两次）；开始收流后中途失败直接报错。
/// - 写入经 `tokio::io::AsyncWrite`（S-R7：Windows 用 `tokio::fs::File` 包装，不在 async 上下文做阻塞 IO）。
/// - 返回写入端（调用方负责 `sync_all` / 关闭）。出错时半截文件由调用方清理。
/// - S-14：记录这次下载实际走了本机代理（端口）还是直连。
async fn stream_asset_into<W, F>(
    url: &str,
    current: &str,
    proxy_port: Option<u16>,
    on_progress: ProgressCallback<'_>,
    limits: DownloadLimits,
    open: F,
) -> Result<W, String>
where
    W: tokio::io::AsyncWrite + Unpin,
    F: FnOnce() -> Result<W, String>,
{
    use futures_util::StreamExt;
    use tokio::io::AsyncWriteExt;

    let max_bytes = limits.max_bytes;
    let proxy_port = if is_loopback_url(url) {
        None
    } else {
        proxy_port
    };
    let mut open = Some(open);
    let mut last_err = String::new();
    for attempt_port in [proxy_port, None] {
        if attempt_port.is_none() && proxy_port.is_some() && !last_err.is_empty() {
            crate::append_app_log(&format!(
                "[update] 经本机代理下载失败（{last_err}），改为直连"
            ));
        }
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
        crate::append_app_log(&match attempt_port {
            Some(p) => format!("[update] 下载更新包：经本机代理 127.0.0.1:{p}"),
            None => "[update] 下载更新包：直连".to_string(),
        });
        let total = resp.content_length();
        if let Some(t) = total {
            if t > max_bytes {
                return Err(format!(
                    "安装包过大（Content-Length {t} > {max_bytes} 上限），已拒绝下载"
                ));
            }
            if let Some(want) = limits.expected_size {
                if t != want {
                    return Err(format!(
                        "安装包大小与 release 记录不符（Content-Length {t}，应为 {want}），已拒绝下载"
                    ));
                }
            }
        }
        let open = open.take().ok_or("下载文件只能创建一次")?;
        let mut file = open()?;
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
            downloaded += chunk.len() as u64;
            if downloaded > max_bytes {
                return Err(format!(
                    "安装包过大（已下载 {downloaded} > {max_bytes} 上限），已中止"
                ));
            }
            if let Some(want) = limits.expected_size {
                if downloaded > want {
                    return Err(format!(
                        "安装包大小与 release 记录不符（已下载 {downloaded} > {want}），已中止"
                    ));
                }
            }
            file.write_all(&chunk)
                .await
                .map_err(|e| format!("写入下载文件失败：{e}"))?;

            let percent = total.map(|t| ((downloaded * 100) / t.max(1)).min(100) as u8);
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
        file.flush().await.map_err(|e| format!("落盘失败：{e}"))?;
        if let Some(want) = limits.expected_size {
            if downloaded != want {
                return Err(format!(
                    "安装包大小与 release 记录不符（下载 {downloaded} 字节，应为 {want}）"
                ));
            }
        }
        // 收尾补发一次，确保前端能看到 100%（流末尾可能不足一个百分比间隔）
        if let Some(t) = total {
            let p = ((downloaded * 100) / t.max(1)).min(100) as u8;
            if (p as i64) != last_pct {
                on_progress(DownloadProgress {
                    downloaded,
                    total,
                    percent: Some(p),
                });
            }
        }
        return Ok(file);
    }
    Err(if last_err.is_empty() {
        "无可用的下载路径".to_string()
    } else {
        last_err
    })
}

/// 流式下载到 `dest`（创建 / 截断）；失败时删除半截文件。Mac DMG 链与单测使用。
#[cfg_attr(windows, allow(dead_code))]
async fn stream_asset_to_file(
    url: &str,
    current: &str,
    dest: &Path,
    proxy_port: Option<u16>,
    on_progress: ProgressCallback<'_>,
    limits: DownloadLimits,
) -> Result<(), String> {
    let r = stream_asset_into(url, current, proxy_port, on_progress, limits, || {
        crate::safe_fs::create_truncate(dest)
            .map(tokio::fs::File::from_std)
            .map_err(|e| format!("创建下载文件失败：{e}"))
    })
    .await;
    match r {
        Ok(mut f) => {
            use tokio::io::AsyncWriteExt;
            f.shutdown().await.map_err(|e| format!("落盘失败：{e}"))?;
            Ok(())
        }
        Err(e) => {
            let _ = std::fs::remove_file(dest);
            Err(e)
        }
    }
}

/// 计算 SHA-256（M-4：在调用方持有的句柄上算，不按路径重新打开），返回小写十六进制。
pub fn sha256_of_reader(r: &mut impl std::io::Read) -> std::io::Result<String> {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = r.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

/// 当前构建的安装形态。
fn current_install_kind() -> InstallKind {
    #[cfg(windows)]
    {
        windows::install_kind()
    }
    #[cfg(not(windows))]
    {
        InstallKind::Dmg
    }
}

/// 测试构建：`CLASH_ASSIST_FAKE_LATEST=x.y.z` 跳过网络，只用来让界面显示更新入口（不提供一键更新）。
#[cfg(any(debug_assertions, feature = "updater-e2e"))]
fn fake_latest(current: &str, install_kind: InstallKind) -> Option<Result<UpdateInfo, String>> {
    let fake = normalize(&std::env::var(FAKE_LATEST_ENV).ok()?);
    if fake.is_empty() {
        return None;
    }
    if parse_version(&fake).is_none() {
        return Some(Err(format!("{FAKE_LATEST_ENV} 不是合法版本号：{fake}")));
    }
    Some(Ok(UpdateInfo {
        current: current.to_string(),
        update_available: is_newer(&fake, current),
        url: format!("{RELEASES_PAGE}/tag/v{fake}"),
        dmg_url: None,
        dmg_sha256: None,
        latest: fake,
        fake: true,
        install_kind,
        one_click: false,
        pending: None,
        updated: None,
        asset: None,
    }))
}

/// 上次更新的对账结果（Windows：启动时由 `pending::reconcile_on_startup` 算一次；其他平台没有）。
async fn startup_notice() -> pending::Outcome {
    #[cfg(windows)]
    {
        tauri::async_runtime::spawn_blocking(pending::startup_outcome)
            .await
            .unwrap_or(pending::Outcome::None)
    }
    #[cfg(not(windows))]
    {
        pending::Outcome::None
    }
}

fn apply_notice(info: &mut UpdateInfo, notice: &pending::Outcome) {
    match notice {
        pending::Outcome::Updated(p) => info.updated = Some(p.clone()),
        pending::Outcome::Unfinished(p) => info.pending = Some(p.clone()),
        pending::Outcome::None => {}
    }
}

pub async fn check(current: &str) -> Result<UpdateInfo, String> {
    let install_kind = current_install_kind();
    let notice = startup_notice().await;
    #[cfg(any(debug_assertions, feature = "updater-e2e"))]
    if let Some(fake) = fake_latest(current, install_kind) {
        return fake.map(|mut info| {
            apply_notice(&mut info, &notice);
            info
        });
    }
    let api = e2e::release_api();
    let fetched = get_bytes_with_proxy_fallback(
        &api,
        current,
        TIMEOUT_MS,
        3000,
        Some("application/vnd.github+json"),
    )
    .await
    .and_then(|bytes| {
        serde_json::from_slice::<serde_json::Value>(&bytes)
            .map_err(|e| format!("解析 GitHub 响应失败：{e}"))
    })
    .and_then(|v| info_from_release(&v, current, AssetKind::current(), install_kind));
    match fetched {
        Ok(mut info) => {
            apply_notice(&mut info, &notice);
            Ok(info)
        }
        // 查不到最新版时，「上次更新未完成 / 已更新」的提示照样给出（不提供一键更新）。
        Err(e) if notice != pending::Outcome::None => {
            crate::append_app_log(&format!("[update] 检查更新失败：{e}"));
            let mut info = UpdateInfo {
                current: current.to_string(),
                latest: current.to_string(),
                update_available: false,
                url: RELEASES_PAGE.to_string(),
                fake: false,
                dmg_url: None,
                dmg_sha256: None,
                install_kind,
                one_click: false,
                pending: None,
                updated: None,
                asset: None,
            };
            apply_notice(&mut info, &notice);
            Ok(info)
        }
        Err(e) => Err(e),
    }
}

/// 阶段 1（下载并校验）。Mac：DMG 到临时目录；Windows：安装包到私有更新目录（M-4 句柄纪律）。
/// 返回工作目录路径（只用于展示 / Mac 的阶段 2；Windows 阶段 3 不接受前端传回的路径）。
pub async fn download_to_staging(on_progress: ProgressCallback<'_>) -> Result<String, String> {
    #[cfg(windows)]
    {
        windows::download_and_verify(on_progress).await
    }
    #[cfg(not(windows))]
    {
        macos::download_to_staging(on_progress).await
    }
}

/// 阶段 2（仅 Mac）：静默挂载 DMG 并替换 .app。Windows 没有这一步（安装程序在阶段 3 运行）。
pub async fn apply_staged_update(work_dir: String) -> Result<String, String> {
    #[cfg(windows)]
    {
        let _ = work_dir;
        Err("Windows 版不使用这一步：下载校验后点「立即重启」由安装程序完成更新".into())
    }
    #[cfg(not(windows))]
    {
        macos::apply_staged_update(work_dir).await
    }
}

/// 阶段 3（用户点击「立即重启」）。Mac：拉起已替换的新版 .app 并退出；
/// Windows：启动已校验的安装程序（`/S /UPDATE /R`），观察 2 s 后退出（M-5）。
pub async fn relaunch_updated_app(app: tauri::AppHandle) -> Result<(), String> {
    #[cfg(windows)]
    {
        windows::launch_installer(app).await
    }
    #[cfg(not(windows))]
    {
        let _ = app;
        macos::relaunch_updated_app()
    }
}

/// 只允许打开本项目的 GitHub release 页面（防止前端被诱导打开任意 URL）。
/// 用 `open` crate 交给系统：macOS 为 `/usr/bin/open -- <url>`、Windows 为 ShellExecuteExW，
/// 均不经 shell 解析（旧实现在 Windows 上用 `cmd /C start`，URL 中的 `&` 等会被 cmd 解释）。
pub fn open_release_page(url: &str) -> Result<(), String> {
    let url = url.trim();
    validate_release_page_url(url)?;
    open::that_detached(url).map_err(|e| format!("打开浏览器失败：{e}"))
}

/// Clash Verge Rev 最新版发布页（Ellie 复核 §4：InsecureDacl 提示里的「升级 Clash Verge 并重启内核」）。
/// 只此一个固定地址：不做前缀匹配，也不接受前端传入的 URL；[`open_release_page`] 的白名单不因此放宽。
pub const VERGE_LATEST_RELEASE_PAGE: &str =
    "https://github.com/clash-verge-rev/clash-verge-rev/releases/latest";

/// 用系统浏览器打开 [`VERGE_LATEST_RELEASE_PAGE`]（无参数，地址编译期固定）。
pub fn open_verge_release_page() -> Result<(), String> {
    open::that_detached(VERGE_LATEST_RELEASE_PAGE).map_err(|e| format!("打开浏览器失败：{e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// M4c 回归：在 tokio 工作线程上（与 `check_update` 命令相同的上下文）探测代理端口不得 panic。
    /// 旧实现在工作线程上同步调用 discovery，Windows 的管道探测内部 `block_on`，panic
    /// "Cannot start a runtime from within a runtime"（release 下即 0xC0000409 闪退）。
    #[test]
    fn proxy_port_probe_is_safe_on_runtime_worker() {
        let r = tauri::async_runtime::block_on(async {
            tauri::async_runtime::spawn(active_local_proxy_port_async()).await
        });
        assert!(r.is_ok(), "在工作线程上探测代理端口 panic：{r:?}");
    }

    /// M4d：同步内核在截止时间过后不再继续发现 / 连接，按未发现端口处理。
    #[test]
    fn proxy_port_probe_stops_after_deadline() {
        use std::cell::RefCell;
        let calls = RefCell::new(Vec::new());
        let r = probe_local_proxy_port(
            Instant::now() + Duration::from_millis(20),
            |client| {
                calls.borrow_mut().push(client.to_string());
                std::thread::sleep(Duration::from_millis(60));
                7999
            },
            |_| panic!("截止时间已过，不应再尝试连接"),
        );
        assert_eq!(r, None);
        assert_eq!(*calls.borrow(), vec!["verge".to_string()]);
        // 已过期的截止时间：一步都不做。
        let r = probe_local_proxy_port(
            Instant::now(),
            |_| panic!("不应调用发现"),
            |_| panic!("不应尝试连接"),
        );
        assert_eq!(r, None);
        // 时间充裕时行为不变：返回第一个可达候选。
        let r = probe_local_proxy_port(
            Instant::now() + Duration::from_secs(60),
            |client| if client == "verge" { 7999 } else { 0 },
            |port| port == 7890,
        );
        assert_eq!(r, Some(7890));
    }

    /// M4d：异步外层总时限——阻塞探测超时即返回 `None`，不等探测结束。
    #[test]
    fn proxy_port_probe_async_budget_times_out() {
        let (r, elapsed) = tauri::async_runtime::block_on(async {
            let start = Instant::now();
            let r = run_probe_with_budget(Duration::from_millis(50), || {
                std::thread::sleep(Duration::from_secs(2));
                Some(7897)
            })
            .await;
            (r, start.elapsed())
        });
        assert_eq!(r, None);
        assert!(
            elapsed < Duration::from_millis(1500),
            "未按时限返回：{elapsed:?}"
        );
        let r =
            tauri::async_runtime::block_on(run_probe_with_budget(Duration::from_secs(30), || {
                Some(7897)
            }));
        assert_eq!(r, Some(7897));
    }

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
        // S-11：预发布序号按数字比较；两段号 / 四段号 / 前导零非法；构建元数据不参与比较
        assert!(is_newer("0.3.0-beta.2", "0.3.0-beta.1"));
        assert!(is_newer("0.3.0-beta.10", "0.3.0-beta.2"));
        assert!(!is_newer("0.3.0-beta.1", "0.3.0-beta.2"));
        assert!(is_newer("0.3.0-rc.1", "0.3.0-beta.9"));
        assert!(!is_newer("0.2", "0.1.0"));
        assert!(!is_newer("0.3.0", "0.2"));
        assert!(!is_newer("0.3.0.1", "0.2.0"));
        assert!(!is_newer("01.0.0", "0.2.0"));
        assert!(!is_newer("0.3.0+build.2", "0.3.0+build.1"));
        assert!(parse_version("0.2").is_none());
        assert!(parse_version(" v0.3.0 ").is_some());
        assert!(same_version("v0.3.0", "0.3.0"));
        assert!(same_version("0.3.0+a", "0.3.0+b"));
        assert!(!same_version("0.3.0-beta.1", "0.3.0"));
    }

    #[test]
    fn only_project_urls_open() {
        assert!(open_release_page("https://evil.example/").is_err());
        assert!(open_release_page("https://github.com/other/repo/releases").is_err());
    }

    #[test]
    fn verge_release_page_is_one_exact_constant() {
        assert_eq!(
            VERGE_LATEST_RELEASE_PAGE,
            "https://github.com/clash-verge-rev/clash-verge-rev/releases/latest"
        );
        let u = url::Url::parse(VERGE_LATEST_RELEASE_PAGE).unwrap();
        assert_eq!(u.scheme(), "https");
        assert_eq!(u.host_str(), Some("github.com"));
        assert_eq!(u.path(), "/clash-verge-rev/clash-verge-rev/releases/latest");
        assert!(u.query().is_none() && u.fragment().is_none());
    }

    #[test]
    fn verge_page_does_not_widen_open_release_page_allowlist() {
        // 通用的 open_release_page 仍只认本项目；Verge 页只能走无参数的 open_verge_release_page
        for url in [
            VERGE_LATEST_RELEASE_PAGE,
            "https://github.com/clash-verge-rev/clash-verge-rev/releases",
            "https://github.com/clash-verge-rev/clash-verge-rev/releases/latest/",
            "https://github.com/clash-verge-rev/clash-verge-rev/releases/tag/v2.0.0",
            "https://github.com/clash-verge-rev/clash-verge-rev/releases/latest?x=1",
        ] {
            assert!(validate_release_page_url(url).is_err(), "{url}");
            assert!(open_release_page(url).is_err(), "{url}");
        }
    }

    const WIN_NAME: &str = "Clash.Assist_0.3.1_x64-setup.exe";
    const WIN_URL: &str =
        "https://github.com/JTee77/clash-assist/releases/download/v0.3.1/Clash.Assist_0.3.1_x64-setup.exe";
    const GOOD_HEX: &str = "251112b7cd87706978a3ad57b15b3d4191dfacc65fefa8d9b5c36da01482dbdc";

    #[test]
    fn asset_url_must_equal_prefix_tag_and_name() {
        assert!(validate_asset_url(WIN_URL, "v0.3.1", WIN_NAME).is_ok());
        for bad in [
            // 路径穿越：旧前缀匹配会放行，解析后实际落到其他仓库
            "https://github.com/JTee77/clash-assist/releases/download/../../../../evil/repo/releases/download/v0.3.1/Clash.Assist_0.3.1_x64-setup.exe",
            "https://github.com/JTee77/clash-assist/releases/download/v0.3.1/../../../../../evil/Clash.Assist_0.3.1_x64-setup.exe",
            // 编码形式的 `.` / `/` / `\`
            "https://github.com/JTee77/clash-assist/releases/download/%2e%2e/%2e%2e/evil/Clash.Assist_0.3.1_x64-setup.exe",
            "https://github.com/JTee77/clash-assist/releases/download/v0.3.1%2fClash.Assist_0.3.1_x64-setup.exe",
            "https://github.com/JTee77/clash-assist/releases/download/v0.3.1%5cClash.Assist_0.3.1_x64-setup.exe",
            "https://github.com/JTee77/clash-assist/releases/download/v0.3.1\\Clash.Assist_0.3.1_x64-setup.exe",
            // 其他 owner / repo（含前缀相似、大小写不同）
            "https://github.com/evil/clash-assist/releases/download/v0.3.1/Clash.Assist_0.3.1_x64-setup.exe",
            "https://github.com/JTee77/clash-assist-evil/releases/download/v0.3.1/Clash.Assist_0.3.1_x64-setup.exe",
            "https://github.com/jtee77/clash-assist/releases/download/v0.3.1/Clash.Assist_0.3.1_x64-setup.exe",
            // http / 其他主机 / 端口 / 用户信息
            "http://github.com/JTee77/clash-assist/releases/download/v0.3.1/Clash.Assist_0.3.1_x64-setup.exe",
            "https://github.com.evil.example/JTee77/clash-assist/releases/download/v0.3.1/Clash.Assist_0.3.1_x64-setup.exe",
            "https://github.com:8443/JTee77/clash-assist/releases/download/v0.3.1/Clash.Assist_0.3.1_x64-setup.exe",
            "https://user@github.com/JTee77/clash-assist/releases/download/v0.3.1/Clash.Assist_0.3.1_x64-setup.exe",
            "https://github.com@evil.example/JTee77/clash-assist/releases/download/v0.3.1/Clash.Assist_0.3.1_x64-setup.exe",
            "http://127.0.0.1:8123/download/v0.3.1/Clash.Assist_0.3.1_x64-setup.exe",
            // 查询串 / 片段 / 额外段 / 其他 tag / 其他文件名 / 大小写
            "https://github.com/JTee77/clash-assist/releases/download/v0.3.1/Clash.Assist_0.3.1_x64-setup.exe?x=1",
            "https://github.com/JTee77/clash-assist/releases/download/v0.3.1/Clash.Assist_0.3.1_x64-setup.exe#f",
            "https://github.com/JTee77/clash-assist/releases/download/v0.3.1/extra/Clash.Assist_0.3.1_x64-setup.exe",
            "https://github.com/JTee77/clash-assist/releases/download/v0.3.0/Clash.Assist_0.3.1_x64-setup.exe",
            "https://github.com/JTee77/clash-assist/releases/download/v0.3.1/Clash.Assist_0.3.0_x64-setup.exe",
            "https://github.com/JTee77/clash-assist/releases/download/v0.3.1/clash.assist_0.3.1_x64-setup.exe",
            "https://github.com/JTee77/clash-assist/releases/download/v0.3.1/Clash.Assist_0.3.1_x64-setup.exe/",
            "https://github.com/JTee77/clash-assist/archive/v0.3.1/Clash.Assist_0.3.1_x64-setup.exe",
            "",
            "not a url",
        ] {
            assert!(validate_asset_url(bad, "v0.3.1", WIN_NAME).is_err(), "should reject: {bad}");
        }
        // tag / 文件名本身不安全
        assert!(validate_asset_url(WIN_URL, "..", WIN_NAME).is_err());
        assert!(validate_asset_url(WIN_URL, "v0.3.1", ".hidden").is_err());
    }

    /// 一份最小的、通过全部校验的 release（Windows + Mac 资产）。
    fn release(version: &str) -> serde_json::Value {
        let tag = format!("v{version}");
        let asset = |name: String| {
            serde_json::json!({
                "name": name,
                "state": "uploaded",
                "size": 1234,
                "digest": format!("sha256:{GOOD_HEX}"),
                "browser_download_url": format!("{DOWNLOAD_PREFIX}{tag}/{name}"),
            })
        };
        serde_json::json!({
            "tag_name": tag,
            "html_url": format!("{RELEASES_PAGE}/tag/{tag}"),
            "draft": false,
            "prerelease": false,
            "immutable": true,
            "assets": [
                asset(AssetKind::MacDmgAarch64.expected_name(version)),
                asset(AssetKind::WinSetupX64.expected_name(version)),
                asset("SHA256SUMS.txt".into()),
            ]
        })
    }

    #[test]
    fn parse_release_asset_accepts_valid_release() {
        let v = release("0.3.1");
        let a = parse_release_asset(&v, AssetKind::WinSetupX64).unwrap();
        assert_eq!(a.name, WIN_NAME);
        assert_eq!(a.url, WIN_URL);
        assert_eq!(a.tag, "v0.3.1");
        assert_eq!(a.version, "0.3.1");
        assert_eq!(a.sha256, GOOD_HEX);
        assert_eq!(a.size, 1234);
        let m = parse_release_asset(&v, AssetKind::MacDmgAarch64).unwrap();
        assert_eq!(m.name, "Clash.Assist_0.3.1_aarch64.dmg");
        // 预发布版本号的 release（prerelease 字段为 false 时）同样按名字精确匹配
        let b = parse_release_asset(&release("0.3.0-beta.2"), AssetKind::WinSetupX64).unwrap();
        assert_eq!(b.name, "Clash.Assist_0.3.0-beta.2_x64-setup.exe");
    }

    /// 改一处字段后应被拒绝（S-5 ②、C-3、V8）。
    fn rejects(mutate: impl Fn(&mut serde_json::Value), why: &str) {
        let mut v = release("0.3.1");
        mutate(&mut v);
        let r = parse_release_asset(&v, AssetKind::WinSetupX64);
        assert!(r.is_err(), "{why}: should reject, got {r:?}");
    }

    fn win_asset(v: &mut serde_json::Value) -> &mut serde_json::Value {
        &mut v["assets"][1]
    }

    #[test]
    fn parse_release_asset_rejects_each_violation() {
        // tag
        rejects(|v| v["tag_name"] = "0.3.1".into(), "tag 缺 v");
        rejects(|v| v["tag_name"] = "v0.3".into(), "两段号");
        rejects(|v| v["tag_name"] = "release-0.3.1".into(), "非版本 tag");
        rejects(
            |v| {
                v.as_object_mut().unwrap().remove("tag_name");
            },
            "缺 tag_name",
        );
        // draft / prerelease：必须存在且为 false
        rejects(|v| v["draft"] = true.into(), "draft");
        rejects(|v| v["prerelease"] = true.into(), "prerelease");
        rejects(
            |v| {
                v.as_object_mut().unwrap().remove("draft");
            },
            "缺 draft",
        );
        rejects(
            |v| {
                v.as_object_mut().unwrap().remove("prerelease");
            },
            "缺 prerelease",
        );
        rejects(|v| v["draft"] = "false".into(), "draft 为字符串");
        // C-3：immutable 缺失 / null / false / 字符串 "true" / 数字 1 一律拒绝
        rejects(
            |v| {
                v.as_object_mut().unwrap().remove("immutable");
            },
            "缺 immutable",
        );
        rejects(
            |v| v["immutable"] = serde_json::Value::Null,
            "immutable null",
        );
        rejects(|v| v["immutable"] = false.into(), "immutable false");
        rejects(|v| v["immutable"] = "true".into(), "immutable 字符串");
        rejects(|v| v["immutable"] = 1.into(), "immutable 数字");
        // 资产名：必须与 V8 完全一致
        rejects(
            |v| win_asset(v)["name"] = "Clash.Assist_0.3.0_x64-setup.exe".into(),
            "名字版本不符",
        );
        rejects(
            |v| win_asset(v)["name"] = "clash.assist_0.3.1_x64-setup.exe".into(),
            "大小写",
        );
        rejects(
            |v| {
                v["assets"] = serde_json::json!([]);
            },
            "没有资产",
        );
        rejects(
            |v| {
                v.as_object_mut().unwrap().remove("assets");
            },
            "缺 assets",
        );
        // state / size / digest
        rejects(|v| win_asset(v)["state"] = "starter".into(), "未上传完成");
        rejects(
            |v| {
                win_asset(v).as_object_mut().unwrap().remove("state");
            },
            "缺 state",
        );
        rejects(
            |v| {
                win_asset(v).as_object_mut().unwrap().remove("size");
            },
            "缺 size",
        );
        rejects(|v| win_asset(v)["size"] = 0.into(), "size 为 0");
        rejects(|v| win_asset(v)["size"] = "1234".into(), "size 为字符串");
        rejects(
            |v| win_asset(v)["size"] = (65u64 << 20).into(),
            "超过 64 MiB",
        );
        rejects(
            |v| {
                win_asset(v).as_object_mut().unwrap().remove("digest");
            },
            "缺 digest",
        );
        rejects(
            |v| win_asset(v)["digest"] = serde_json::Value::Null,
            "digest null",
        );
        rejects(
            |v| win_asset(v)["digest"] = GOOD_HEX.into(),
            "digest 缺 sha256: 前缀",
        );
        rejects(
            |v| win_asset(v)["digest"] = "md5:abc".into(),
            "digest 算法不对",
        );
        rejects(
            |v| win_asset(v)["digest"] = format!("sha256:{}", &GOOD_HEX[..63]).into(),
            "digest 长度",
        );
        rejects(
            |v| win_asset(v)["digest"] = format!("sha256:{}z", &GOOD_HEX[..63]).into(),
            "digest 非十六进制",
        );
        // 下载地址
        rejects(
            |v| win_asset(v)["browser_download_url"] = "https://evil.example/x.exe".into(),
            "其他主机",
        );
        rejects(
            |v| {
                win_asset(v)["browser_download_url"] =
                    "https://github.com/JTee77/clash-assist/releases/download/v0.1.0/Clash.Assist_0.3.1_x64-setup.exe".into()
            },
            "tag 段不符",
        );
        // 只有 API `url` 字段（非 browser_download_url）不当作下载直链
        rejects(
            |v| {
                let a = win_asset(v).as_object_mut().unwrap();
                let u = a.remove("browser_download_url").unwrap();
                a.insert("url".into(), u);
            },
            "缺 browser_download_url",
        );
    }

    /// S-R6：真实 GitHub release API 响应（tests/fixtures/github-release/，取自本仓库已发布 release
    /// 与 mihomo v1.19.33，字段形态原样保留）。
    const FIXTURE_LATEST: &str =
        include_str!("../../../tests/fixtures/github-release/clash-assist-latest.json");
    const FIXTURE_RELEASES: &str =
        include_str!("../../../tests/fixtures/github-release/clash-assist-releases.json");
    const FIXTURE_MIHOMO: &str =
        include_str!("../../../tests/fixtures/github-release/mihomo-v1.19.33.json");

    #[test]
    fn real_release_json_without_immutability_is_refused() {
        // 0.2.x 都是在开启 immutable releases 之前发布的：immutable 为 false → 拒绝一键更新（C-3）
        let latest: serde_json::Value = serde_json::from_str(FIXTURE_LATEST).unwrap();
        assert_eq!(latest["immutable"], serde_json::Value::Bool(false));
        let err = parse_release_asset(&latest, AssetKind::MacDmgAarch64).unwrap_err();
        assert!(err.contains("immutable"), "{err}");
        let all: Vec<serde_json::Value> = serde_json::from_str(FIXTURE_RELEASES).unwrap();
        assert!(all.len() > 20);
        for r in &all {
            assert!(
                parse_release_asset(r, AssetKind::MacDmgAarch64).is_err(),
                "{}",
                r["tag_name"]
            );
            assert!(
                parse_release_asset(r, AssetKind::WinSetupX64).is_err(),
                "{}",
                r["tag_name"]
            );
        }
        // 仍能得到 UpdateInfo（显示「去下载」），只是没有一键更新
        let info = info_from_release(&latest, "0.2.8", AssetKind::MacDmgAarch64, InstallKind::Dmg)
            .unwrap();
        assert!(info.update_available);
        assert!(!info.one_click);
        assert!(info.asset.is_none() && info.dmg_url.is_none());
        assert_eq!(
            info.url,
            "https://github.com/JTee77/clash-assist/releases/tag/v0.2.9"
        );
    }

    /// 真实 JSON 只把 `immutable` 改成 true（其余字段原样）→ Mac 资产通过；没有 Windows 资产 → Windows 拒绝。
    #[test]
    fn real_release_json_parses_once_immutable() {
        let mut latest: serde_json::Value = serde_json::from_str(FIXTURE_LATEST).unwrap();
        latest["immutable"] = true.into();
        let a = parse_release_asset(&latest, AssetKind::MacDmgAarch64).unwrap();
        assert_eq!(a.name, "Clash.Assist_0.2.9_aarch64.dmg");
        assert_eq!(a.size, latest["assets"][0]["size"].as_u64().unwrap());
        assert_eq!(
            format!("sha256:{}", a.sha256),
            latest["assets"][0]["digest"].as_str().unwrap()
        );
        let err = parse_release_asset(&latest, AssetKind::WinSetupX64).unwrap_err();
        assert!(err.contains("x64-setup.exe"), "{err}");
        // 历史 release：每一个改成 immutable 后都能解析出 V8 命名的 DMG（有 digest 的那些）
        let all: Vec<serde_json::Value> = serde_json::from_str(FIXTURE_RELEASES).unwrap();
        let mut ok = 0;
        for mut r in all {
            r["immutable"] = true.into();
            let has_digest = r["assets"].as_array().unwrap().iter().any(|x| {
                x["digest"]
                    .as_str()
                    .is_some_and(|d| d.starts_with("sha256:"))
            });
            let parsed = parse_release_asset(&r, AssetKind::MacDmgAarch64);
            if has_digest && parsed.is_ok() {
                ok += 1;
            }
        }
        assert!(ok >= 3, "至少近几个版本应可解析：{ok}");
    }

    /// 用 mihomo 的真实 release（127 个资产、字段形态与 GitHub 实际返回一致）改写出一份
    /// Windows release：只替换 tag、资产名、URL（其余字段原样），验证解析器不被真实形态绊倒。
    #[test]
    fn real_mihomo_shape_rewritten_to_windows_release() {
        let mut v: serde_json::Value = serde_json::from_str(FIXTURE_MIHOMO).unwrap();
        assert!(v["assets"].as_array().unwrap().len() > 100);
        v["tag_name"] = "v0.3.1".into();
        v["immutable"] = true.into();
        let a0 = &mut v["assets"][0];
        let size = a0["size"].as_u64().unwrap();
        let digest = a0["digest"].as_str().unwrap().to_string();
        a0["name"] = WIN_NAME.into();
        a0["browser_download_url"] = WIN_URL.into();
        let a = parse_release_asset(&v, AssetKind::WinSetupX64).unwrap();
        assert_eq!(a.size, size);
        assert_eq!(format!("sha256:{}", a.sha256), digest);
        // 原样的 mihomo JSON：资产名、URL 都不属于本项目 → 拒绝
        let raw: serde_json::Value = serde_json::from_str(FIXTURE_MIHOMO).unwrap();
        assert!(parse_release_asset(&raw, AssetKind::WinSetupX64).is_err());
    }

    #[test]
    fn info_from_release_one_click_rules() {
        let v = release("0.3.1");
        let win =
            info_from_release(&v, "0.3.0", AssetKind::WinSetupX64, InstallKind::Nsis).unwrap();
        assert!(win.update_available && win.one_click);
        assert_eq!(win.asset.as_ref().unwrap().name, WIN_NAME);
        assert!(win.dmg_url.is_none() && win.dmg_sha256.is_none());
        // 便携版：有更新，但不提供一键更新
        let portable =
            info_from_release(&v, "0.3.0", AssetKind::WinSetupX64, InstallKind::Portable).unwrap();
        assert!(portable.update_available && !portable.one_click);
        assert!(resolve_download_target(&portable).is_err());
        // 已是最新
        let same =
            info_from_release(&v, "0.3.1", AssetKind::WinSetupX64, InstallKind::Nsis).unwrap();
        assert!(!same.update_available && !same.one_click);
        // Mac：dmg_url / dmg_sha256 照旧给出（CLI 与旧前端兼容）
        let mac =
            info_from_release(&v, "0.3.0", AssetKind::MacDmgAarch64, InstallKind::Dmg).unwrap();
        assert!(mac.one_click);
        assert_eq!(
            mac.dmg_sha256.as_deref(),
            Some(format!("sha256:{GOOD_HEX}").as_str())
        );
        // 序列化：install_kind 小写；asset 不发给前端
        let j = serde_json::to_value(&win).unwrap();
        assert_eq!(j["install_kind"], "nsis");
        assert_eq!(j["one_click"], true);
        assert!(j.get("asset").is_none());
        assert!(j.get("pending").is_none() && j.get("updated").is_none());
    }

    #[test]
    fn download_target_fails_closed() {
        let v = release("0.3.1");
        let ok = info_from_release(&v, "0.3.0", AssetKind::WinSetupX64, InstallKind::Nsis).unwrap();
        let a = resolve_download_target(&ok).unwrap();
        assert_eq!(a.url, WIN_URL);
        assert_eq!(a.sha256, GOOD_HEX);
        // 没有更新
        let mut no_update = ok.clone();
        no_update.update_available = false;
        assert!(resolve_download_target(&no_update).is_err());
        // 没有通过校验的资产（例如 release 不是 immutable）
        let mut no_asset = ok.clone();
        no_asset.asset = None;
        let e = resolve_download_target(&no_asset).unwrap_err();
        assert!(e.contains("安全校验"), "{e}");
        // 资产地址被改过（防御性复查）
        let mut tampered = ok.clone();
        tampered.asset.as_mut().unwrap().url = "https://evil.example/x.exe".into();
        assert!(resolve_download_target(&tampered).is_err());
        // 资产版本不新于当前版本（降级）
        let mut downgrade = ok.clone();
        downgrade.current = "0.3.2".into();
        assert!(resolve_download_target(&downgrade).is_err());
    }

    #[test]
    fn sha256_of_reader_matches_known_vectors() {
        let mut r = std::io::Cursor::new(b"abc".to_vec());
        assert_eq!(
            sha256_of_reader(&mut r).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let mut empty = std::io::Cursor::new(Vec::new());
        assert_eq!(
            sha256_of_reader(&mut empty).unwrap(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        // 跨越 64 KiB 读块边界
        let big = vec![b'a'; (1 << 16) + 7];
        let mut r = std::io::Cursor::new(big.clone());
        use sha2::{Digest, Sha256};
        let want: String = Sha256::digest(&big)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(sha256_of_reader(&mut r).unwrap(), want);
    }

    /// M-7：FAKE_LATEST 只存在于 debug / updater-e2e 构建（release 构建由 scan-exe-markers.ps1 反向断言）。
    #[test]
    fn fake_latest_is_test_build_only() {
        assert_eq!(FAKE_LATEST_ENV, "CLASH_ASSIST_FAKE_LATEST");
        std::env::set_var(FAKE_LATEST_ENV, "99.0.0");
        let r = fake_latest("0.3.0", InstallKind::Nsis).unwrap().unwrap();
        std::env::remove_var(FAKE_LATEST_ENV);
        assert!(r.fake && r.update_available);
        // 假版本永远不提供一键更新
        assert!(!r.one_click && r.asset.is_none());
        assert!(resolve_download_target(&r).is_err());
    }

    #[test]
    fn scan_positive_control_is_kept() {
        keep_build_markers();
        assert_eq!(
            &SCAN_POSITIVE_CONTROL,
            b"CLASH_ASSIST_SCAN_POSITIVE_CONTROL"
        );
    }

    #[test]
    fn pending_notice_is_attached() {
        let v = release("0.3.1");
        let mut info =
            info_from_release(&v, "0.3.0", AssetKind::WinSetupX64, InstallKind::Nsis).unwrap();
        let pair = VersionPair {
            from: "0.3.0".into(),
            to: "0.3.1".into(),
        };
        apply_notice(&mut info, &pending::Outcome::Unfinished(pair.clone()));
        assert_eq!(info.pending, Some(pair.clone()));
        let j = serde_json::to_value(&info).unwrap();
        assert_eq!(j["pending"]["from"], "0.3.0");
        assert_eq!(j["pending"]["to"], "0.3.1");
        let mut info2 =
            info_from_release(&v, "0.3.1", AssetKind::WinSetupX64, InstallKind::Nsis).unwrap();
        apply_notice(&mut info2, &pending::Outcome::Updated(pair.clone()));
        assert_eq!(info2.updated, Some(pair));
        assert!(info2.pending.is_none());
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
        tauri::async_runtime::block_on(stream_asset_to_file(
            &url,
            "test",
            &dest,
            None, // 不走本地代理探测，直连本地测试服务器
            &on_progress,
            DownloadLimits {
                max_bytes: 512 << 20,
                expected_size: Some(TOTAL as u64),
            },
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
        let r = tauri::async_runtime::block_on(stream_asset_to_file(
            &url,
            "test",
            &dest,
            None,
            &|_| {},
            DownloadLimits {
                max_bytes: 1024,
                expected_size: None,
            },
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
        let r = tauri::async_runtime::block_on(stream_asset_to_file(
            &url,
            "test",
            &dest,
            None,
            &|_| {},
            DownloadLimits {
                max_bytes: 2048,
                expected_size: None,
            },
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

    /// M-4：下载字节数必须恰好等于 API 的 `size`（Content-Length 不符直接拒绝；分块流超出即中止）。
    #[test]
    fn download_enforces_expected_size() {
        let resp = b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: close\r\n\r\nabc".to_vec();
        let (port, h) = serve_raw_once(resp);
        let dir = dmg_test_dir("size-cl");
        let dest = dir.join("t.bin");
        let r = tauri::async_runtime::block_on(stream_asset_to_file(
            &format!("http://127.0.0.1:{port}/t.bin"),
            "test",
            &dest,
            None,
            &|_| {},
            DownloadLimits {
                max_bytes: 1 << 20,
                expected_size: Some(4),
            },
        ));
        let _ = h.join();
        assert!(r.unwrap_err().contains("大小"));
        assert!(!crate::safe_fs::exists(&dest), "nothing should be written");

        // chunked：实际 2 KiB，API 说 1 KiB → 超出即中止并删掉半截文件
        let (port, h) = serve_raw_once(chunked_body(2, 1024));
        let r = tauri::async_runtime::block_on(stream_asset_to_file(
            &format!("http://127.0.0.1:{port}/t.bin"),
            "test",
            &dest,
            None,
            &|_| {},
            DownloadLimits {
                max_bytes: 1 << 20,
                expected_size: Some(1024),
            },
        ));
        let _ = h.join();
        assert!(r.unwrap_err().contains("大小"));
        assert!(!crate::safe_fs::exists(&dest));

        // chunked：实际 2 KiB，API 说 3 KiB → 流结束后不足也拒绝
        let (port, h) = serve_raw_once(chunked_body(2, 1024));
        let r = tauri::async_runtime::block_on(stream_asset_to_file(
            &format!("http://127.0.0.1:{port}/t.bin"),
            "test",
            &dest,
            None,
            &|_| {},
            DownloadLimits {
                max_bytes: 1 << 20,
                expected_size: Some(3072),
            },
        ));
        let _ = h.join();
        assert!(r.unwrap_err().contains("大小"));
        assert!(!crate::safe_fs::exists(&dest));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
