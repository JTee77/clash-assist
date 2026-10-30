//! macOS 应用菜单：与 Tauri 默认菜单一致，但「关于」面板不传硬编码的 productName，
//! 让 AppKit 使用 InfoPlist.strings 中的本地化名称。
//! 0.2.1：菜单文字覆盖 简体 / 繁體 / English 三种语言；默认跟随系统首选语言，
//! 前端手动切换语言时经 `set_ui_language` 覆盖并重建菜单。
use std::sync::Mutex;

use tauri::menu::{AboutMetadata, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Runtime};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiLang {
    Hans,
    Hant,
    En,
}

/// 前端手动选择的语言；None = 跟随系统。
static OVERRIDE: Mutex<Option<UiLang>> = Mutex::new(None);

/// BCP-47 / Apple locale 标签 → UiLang：zh-Hant / zh-TW / zh-HK / zh-MO → 繁體；其它 zh* → 简体；其余 → English。
pub fn lang_from_tag(tag: &str) -> UiLang {
    let l = tag.to_ascii_lowercase().replace('_', "-");
    if !(l == "zh" || l.starts_with("zh-")) {
        return UiLang::En;
    }
    let parts: Vec<&str> = l.split('-').collect();
    if parts.contains(&"hant")
        || (parts.iter().any(|p| matches!(*p, "tw" | "hk" | "mo")) && !parts.contains(&"hans"))
    {
        UiLang::Hant
    } else {
        UiLang::Hans
    }
}

/// 系统首选语言标签（macOS 上 sys-locale 读取 NSLocale.preferredLanguages）。
pub fn system_locale_tag() -> String {
    sys_locale::get_locales()
        .next()
        .or_else(sys_locale::get_locale)
        .unwrap_or_else(|| "en".to_string())
}

pub fn parse_pref(pref: &str) -> Option<UiLang> {
    match pref {
        "zh-Hans" => Some(UiLang::Hans),
        "zh-Hant" => Some(UiLang::Hant),
        "en" => Some(UiLang::En),
        _ => None, // "system" 或未知值 → 跟随系统
    }
}

pub fn set_override(lang: Option<UiLang>) {
    if let Ok(mut g) = OVERRIDE.lock() {
        *g = lang;
    }
}

pub fn current_lang() -> UiLang {
    OVERRIDE
        .lock()
        .ok()
        .and_then(|g| *g)
        .unwrap_or_else(|| lang_from_tag(&system_locale_tag()))
}

pub fn build<R: Runtime>(h: &AppHandle<R>) -> tauri::Result<Menu<R>> {
    let lang = current_lang();
    let pick = |hans: &str, hant: &str, en: &str| -> String {
        match lang {
            UiLang::Hans => hans,
            UiLang::Hant => hant,
            UiLang::En => en,
        }
        .to_string()
    };
    let t = |hans: &str, hant: &str, en: &str| -> Option<String> { Some(pick(hans, hant, en)) };
    let about = AboutMetadata {
        name: None, // 由系统取本地化 CFBundleName / CFBundleDisplayName
        version: Some(h.package_info().version.to_string()),
        copyright: h.config().bundle.copyright.clone(),
        authors: h.config().bundle.publisher.clone().map(|p| vec![p]),
        ..Default::default()
    };
    // 子菜单标题会被 AppKit 以本地化 bundle 名显示，这里的文字不可见。
    let app_menu = Submenu::with_items(
        h,
        h.package_info().name.clone(),
        true,
        &[
            &PredefinedMenuItem::about(
                h,
                t("关于 Clash伴侣", "關於 Clash伴侶", "About Clash Assist").as_deref(),
                Some(about),
            )?,
            &PredefinedMenuItem::separator(h)?,
            &PredefinedMenuItem::services(h, t("服务", "服務", "Services").as_deref())?,
            &PredefinedMenuItem::separator(h)?,
            &PredefinedMenuItem::hide(
                h,
                t("隐藏 Clash伴侣", "隱藏 Clash伴侶", "Hide Clash Assist").as_deref(),
            )?,
            &PredefinedMenuItem::hide_others(
                h,
                t("隐藏其他", "隱藏其他", "Hide Others").as_deref(),
            )?,
            &PredefinedMenuItem::separator(h)?,
            &PredefinedMenuItem::quit(
                h,
                t("退出 Clash伴侣", "結束 Clash伴侶", "Quit Clash Assist").as_deref(),
            )?,
        ],
    )?;
    let close = || {
        PredefinedMenuItem::close_window(h, t("关闭窗口", "關閉視窗", "Close Window").as_deref())
    };
    let file = Submenu::with_items(h, pick("文件", "檔案", "File"), true, &[&close()?])?;
    let edit = Submenu::with_items(
        h,
        pick("编辑", "編輯", "Edit"),
        true,
        &[
            &PredefinedMenuItem::undo(h, t("撤销", "還原", "Undo").as_deref())?,
            &PredefinedMenuItem::redo(h, t("重做", "重做", "Redo").as_deref())?,
            &PredefinedMenuItem::separator(h)?,
            &PredefinedMenuItem::cut(h, t("剪切", "剪下", "Cut").as_deref())?,
            &PredefinedMenuItem::copy(h, t("拷贝", "拷貝", "Copy").as_deref())?,
            &PredefinedMenuItem::paste(h, t("粘贴", "貼上", "Paste").as_deref())?,
            &PredefinedMenuItem::select_all(h, t("全选", "全選", "Select All").as_deref())?,
        ],
    )?;
    let reload_item = MenuItem::with_id(
        h,
        "reload_window",
        pick("重新加载", "重新載入", "Reload"),
        true,
        Some("CmdOrCtrl+R"),
    )?;
    let view = Submenu::with_items(
        h,
        pick("显示", "顯示方式", "View"),
        true,
        &[
            &reload_item,
            &PredefinedMenuItem::separator(h)?,
            &PredefinedMenuItem::fullscreen(
                h,
                t("进入全屏幕", "進入全螢幕", "Enter Full Screen").as_deref(),
            )?,
        ],
    )?;
    let window = Submenu::with_id_and_items(
        h,
        tauri::menu::WINDOW_SUBMENU_ID,
        pick("窗口", "視窗", "Window"),
        true,
        &[
            &PredefinedMenuItem::minimize(h, t("最小化", "縮到最小", "Minimize").as_deref())?,
            &PredefinedMenuItem::maximize(h, t("缩放", "縮放", "Zoom").as_deref())?,
            &PredefinedMenuItem::separator(h)?,
            &close()?,
        ],
    )?;
    let help = Submenu::with_id_and_items(
        h,
        tauri::menu::HELP_SUBMENU_ID,
        pick("帮助", "輔助說明", "Help"),
        true,
        &[],
    )?;
    Menu::with_items(h, &[&app_menu, &file, &edit, &view, &window, &help])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tag_mapping() {
        assert_eq!(lang_from_tag("zh-Hans-CN"), UiLang::Hans);
        assert_eq!(lang_from_tag("zh-CN"), UiLang::Hans);
        assert_eq!(lang_from_tag("zh"), UiLang::Hans);
        assert_eq!(lang_from_tag("zh-Hant-TW"), UiLang::Hant);
        assert_eq!(lang_from_tag("zh-TW"), UiLang::Hant);
        assert_eq!(lang_from_tag("zh_HK"), UiLang::Hant);
        assert_eq!(lang_from_tag("zh-Hans-HK"), UiLang::Hans);
        assert_eq!(lang_from_tag("en-US"), UiLang::En);
        assert_eq!(lang_from_tag("ja-JP"), UiLang::En);
    }

    #[test]
    fn pref_parse() {
        assert_eq!(parse_pref("system"), None);
        assert_eq!(parse_pref("zh-Hant"), Some(UiLang::Hant));
        assert_eq!(parse_pref("en"), Some(UiLang::En));
    }
}
