//! `updater-e2e` feature 专用开关（windows-plan r4 §2.4「测试专用 feature」，M-7）。
//!
//! release workflow **绝不启用**这个 feature。启用后只多出三件事：
//! 1. 环境变量 `CLASH_ASSIST_E2E_API` 可把 latest release API 指向
//!    `http://127.0.0.1:<port>/release.json`（只认这个形状）；
//! 2. 下载地址白名单放宽到同一个本地源：`http://127.0.0.1:<port>/download/<tag>/<资产名>`；
//! 3. 编译进标记常量 `CLASH_ASSIST_UPDATER_E2E_BUILD`（`#[used]` + 启动时 `black_box`），
//!    `updater-e2e.yml` 对 e2e 构建做正向断言、release 构建做反向断言（S-R4）。
//!
//! digest、size、immutable 等校验照旧从 JSON 读取，**校验逻辑一行不改**。
//! 不启用 feature 时，下面的函数退化为常量结果，环境变量名与标记字符串都不会进入 exe。

#[cfg(feature = "updater-e2e")]
mod imp {
    pub const API_ENV: &str = "CLASH_ASSIST_E2E_API";

    #[used]
    static UPDATER_E2E_BUILD_MARKER: [u8; 30] = *b"CLASH_ASSIST_UPDATER_E2E_BUILD";

    pub fn keep_marker() {
        std::hint::black_box(UPDATER_E2E_BUILD_MARKER.as_ptr());
    }

    /// 只接受 `http://127.0.0.1:<port>/release.json`（无用户信息、查询串、片段）。
    pub fn parse_api(raw: &str) -> Option<url::Url> {
        let u = url::Url::parse(raw.trim()).ok()?;
        let ok = u.scheme() == "http"
            && u.host_str() == Some("127.0.0.1")
            && u.port().is_some()
            && u.username().is_empty()
            && u.password().is_none()
            && u.path() == "/release.json"
            && u.query().is_none()
            && u.fragment().is_none();
        ok.then_some(u)
    }

    pub fn api_override() -> Option<url::Url> {
        parse_api(&std::env::var(API_ENV).ok()?)
    }

    pub fn local_asset_url_ok_for(api: &url::Url, url: &str, tag: &str, name: &str) -> bool {
        let port = match api.port() {
            Some(p) => p,
            None => return false,
        };
        url == format!("http://127.0.0.1:{port}/download/{tag}/{name}")
    }
}

/// latest release API 地址：生产构建恒为 `LATEST_RELEASE_API`。
pub fn release_api() -> String {
    #[cfg(feature = "updater-e2e")]
    if let Some(u) = imp::api_override() {
        crate::append_app_log(&format!("[update] e2e：release API 指向 {u}"));
        return u.to_string();
    }
    super::LATEST_RELEASE_API.to_string()
}

/// e2e 本地源下的资产地址（`tag`、`name` 已由调用方校验为安全路径段）。生产构建恒为 false。
pub fn local_asset_url_ok(url: &str, tag: &str, name: &str) -> bool {
    #[cfg(feature = "updater-e2e")]
    {
        imp::api_override().is_some_and(|api| imp::local_asset_url_ok_for(&api, url, tag, name))
    }
    #[cfg(not(feature = "updater-e2e"))]
    {
        let _ = (url, tag, name);
        false
    }
}

pub fn keep_marker() {
    #[cfg(feature = "updater-e2e")]
    imp::keep_marker();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 非 e2e 构建：没有任何覆盖（环境变量名本身都不存在于代码中，这里用字面量模拟设置）。
    #[cfg(not(feature = "updater-e2e"))]
    #[test]
    fn production_build_has_no_overrides() {
        std::env::set_var("CLASH_ASSIST_E2E_API", "http://127.0.0.1:8123/release.json");
        assert_eq!(release_api(), super::super::LATEST_RELEASE_API);
        assert!(!local_asset_url_ok(
            "http://127.0.0.1:8123/download/v9.9.9/Clash.Assist_9.9.9_x64-setup.exe",
            "v9.9.9",
            "Clash.Assist_9.9.9_x64-setup.exe"
        ));
        assert!(super::super::validate_asset_url(
            "http://127.0.0.1:8123/download/v9.9.9/Clash.Assist_9.9.9_x64-setup.exe",
            "v9.9.9",
            "Clash.Assist_9.9.9_x64-setup.exe"
        )
        .is_err());
        std::env::remove_var("CLASH_ASSIST_E2E_API");
    }

    #[cfg(feature = "updater-e2e")]
    #[test]
    fn e2e_api_shape_is_pinned() {
        assert!(imp::parse_api("http://127.0.0.1:8123/release.json").is_some());
        for bad in [
            "https://127.0.0.1:8123/release.json",
            "http://localhost:8123/release.json",
            "http://127.0.0.1/release.json",
            "http://127.0.0.1:8123/other.json",
            "http://127.0.0.1:8123/release.json?x=1",
            "http://u@127.0.0.1:8123/release.json",
            "http://10.0.0.1:8123/release.json",
        ] {
            assert!(imp::parse_api(bad).is_none(), "{bad}");
        }
        let api = imp::parse_api("http://127.0.0.1:8123/release.json").unwrap();
        let name = "Clash.Assist_0.3.1_x64-setup.exe";
        assert!(imp::local_asset_url_ok_for(
            &api,
            &format!("http://127.0.0.1:8123/download/v0.3.1/{name}"),
            "v0.3.1",
            name
        ));
        assert!(!imp::local_asset_url_ok_for(
            &api,
            &format!("http://127.0.0.1:9999/download/v0.3.1/{name}"),
            "v0.3.1",
            name
        ));
        assert!(!imp::local_asset_url_ok_for(
            &api,
            &format!("http://127.0.0.1:8123/download/v0.3.0/{name}"),
            "v0.3.1",
            name
        ));
    }
}
