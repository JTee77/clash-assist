//! macOS 应用菜单：与 Tauri 默认菜单一致，但「关于」面板不传硬编码的 productName，
//! 让 AppKit 使用 InfoPlist.strings 中的本地化名称（中文系统「Clash伴侣」，其他「Clash Assist」）。
//! 「关于/隐藏/退出 xxx」菜单项文字由 muda 取 NSRunningApplication.localizedName，同样已本地化。
use tauri::menu::{AboutMetadata, Menu, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Runtime};

/// 系统首选语言是否为中文（zh / zh-Hans / zh-Hant / zh-CN ...）。
/// macOS 上 sys-locale 读取 NSLocale.preferredLanguages，与 AppleLanguages 顺序一致。
fn is_zh() -> bool {
    sys_locale::get_locales()
        .next()
        .or_else(sys_locale::get_locale)
        .map(|l| {
            let l = l.to_ascii_lowercase();
            l == "zh" || l.starts_with("zh-") || l.starts_with("zh_")
        })
        .unwrap_or(false)
}

pub fn build<R: Runtime>(h: &AppHandle<R>) -> tauri::Result<Menu<R>> {
    let zh = is_zh();
    // 中文返回 Some(文字)，英文返回 None 以保持 muda 默认文字
    let t = |cn: &str| -> Option<String> { if zh { Some(cn.to_string()) } else { None } };
    let name = if zh { "Clash伴侣" } else { "Clash Assist" };
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
            &PredefinedMenuItem::about(h, t(&format!("关于 {name}")).as_deref(), Some(about))?,
            &PredefinedMenuItem::separator(h)?,
            &PredefinedMenuItem::services(h, t("服务").as_deref())?,
            &PredefinedMenuItem::separator(h)?,
            &PredefinedMenuItem::hide(h, t(&format!("隐藏 {name}")).as_deref())?,
            &PredefinedMenuItem::hide_others(h, t("隐藏其他").as_deref())?,
            &PredefinedMenuItem::separator(h)?,
            &PredefinedMenuItem::quit(h, t(&format!("退出 {name}")).as_deref())?,
        ],
    )?;
    let file = Submenu::with_items(h, if zh { "文件" } else { "File" }, true, &[&PredefinedMenuItem::close_window(h, t("关闭窗口").as_deref())?])?;
    let edit = Submenu::with_items(
        h,
        if zh { "编辑" } else { "Edit" },
        true,
        &[
            &PredefinedMenuItem::undo(h, t("撤销").as_deref())?,
            &PredefinedMenuItem::redo(h, t("重做").as_deref())?,
            &PredefinedMenuItem::separator(h)?,
            &PredefinedMenuItem::cut(h, t("剪切").as_deref())?,
            &PredefinedMenuItem::copy(h, t("拷贝").as_deref())?,
            &PredefinedMenuItem::paste(h, t("粘贴").as_deref())?,
            &PredefinedMenuItem::select_all(h, t("全选").as_deref())?,
        ],
    )?;
    let view = Submenu::with_items(h, if zh { "显示" } else { "View" }, true, &[&PredefinedMenuItem::fullscreen(h, t("进入全屏幕").as_deref())?])?;
    let window = Submenu::with_id_and_items(
        h,
        tauri::menu::WINDOW_SUBMENU_ID,
        if zh { "窗口" } else { "Window" },
        true,
        &[
            &PredefinedMenuItem::minimize(h, t("最小化").as_deref())?,
            &PredefinedMenuItem::maximize(h, t("缩放").as_deref())?,
            &PredefinedMenuItem::separator(h)?,
            &PredefinedMenuItem::close_window(h, t("关闭窗口").as_deref())?,
        ],
    )?;
    let help = Submenu::with_id_and_items(h, tauri::menu::HELP_SUBMENU_ID, if zh { "帮助" } else { "Help" }, true, &[])?;
    Menu::with_items(h, &[&app_menu, &file, &edit, &view, &window, &help])
}
