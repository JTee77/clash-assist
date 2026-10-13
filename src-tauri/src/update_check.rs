//! 更新检查：查询 GitHub latest release，与当前版本做 semver 比较。
//! GUI 启动时调用一次（失败静默）；CLI `update-check` 复用同一逻辑。
//!
//! 测试 / 截图用：环境变量 `CLASH_ASSIST_FAKE_LATEST=0.2.1` 跳过网络，直接当作最新版本。

use serde::Serialize;

pub const LATEST_RELEASE_API: &str =
    "https://api.github.com/repos/JTee77/clash-assist/releases/latest";
pub const RELEASES_PAGE: &str = "https://github.com/JTee77/clash-assist/releases";
pub const FAKE_LATEST_ENV: &str = "CLASH_ASSIST_FAKE_LATEST";
const TIMEOUT_MS: u64 = 5000;

#[derive(Debug, Serialize, Clone)]
pub struct UpdateInfo {
    pub current: String,
    pub latest: String,
    pub update_available: bool,
    pub url: String,
    /// true = 来自 CLASH_ASSIST_FAKE_LATEST（未访问网络）
    pub fake: bool,
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
                latest: fake,
                fake: true,
            });
        }
    }
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_millis(TIMEOUT_MS))
        .connect_timeout(std::time::Duration::from_millis(3000))
        .user_agent(format!("clash-assist/{current}"))
        .build()
        .map_err(|e| format!("http client: {e}"))?;
    let resp = client
        .get(LATEST_RELEASE_API)
        .header("Accept", "application/vnd.github+json")
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
    Ok(UpdateInfo {
        current: current.to_string(),
        update_available: is_newer(&latest, current),
        latest,
        url,
        fake: false,
    })
}

/// 只允许打开本项目的 GitHub release 页面（防止前端被诱导打开任意 URL）。
pub fn open_release_page(url: &str) -> Result<(), String> {
    if !url.starts_with(RELEASES_PAGE) {
        return Err("只允许打开本项目的 release 页面".into());
    }
    #[cfg(target_os = "macos")]
    let mut cmd = std::process::Command::new("open");
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut cmd = std::process::Command::new("xdg-open");
    #[cfg(windows)]
    let mut cmd = {
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", "start", ""]);
        c
    };
    cmd.arg(url).spawn().map(|_| ()).map_err(|e| format!("打开浏览器失败：{e}"))
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
}
