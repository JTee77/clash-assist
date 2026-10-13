//! macOS 应用菜单：与 Tauri 默认菜单一致，但「关于」面板不传硬编码的 productName，
//! 让 AppKit 使用 InfoPlist.strings 中的本地化名称（中文系统「Clash伴侣」，其他「Clash Assist」）。
//! 「关于/隐藏/退出 xxx」菜单项文字由 muda 取 NSRunningApplication.localizedName，同样已本地化。
use tauri::menu::{AboutMetadata, Menu, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Runtime};

pub fn build<R: Runtime>(h: &AppHandle<R>) -> tauri::Result<Menu<R>> {
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
            &PredefinedMenuItem::about(h, None, Some(about))?,
            &PredefinedMenuItem::separator(h)?,
            &PredefinedMenuItem::services(h, None)?,
            &PredefinedMenuItem::separator(h)?,
            &PredefinedMenuItem::hide(h, None)?,
            &PredefinedMenuItem::hide_others(h, None)?,
            &PredefinedMenuItem::separator(h)?,
            &PredefinedMenuItem::quit(h, None)?,
        ],
    )?;
    let file = Submenu::with_items(h, "File", true, &[&PredefinedMenuItem::close_window(h, None)?])?;
    let edit = Submenu::with_items(
        h,
        "Edit",
        true,
        &[
            &PredefinedMenuItem::undo(h, None)?,
            &PredefinedMenuItem::redo(h, None)?,
            &PredefinedMenuItem::separator(h)?,
            &PredefinedMenuItem::cut(h, None)?,
            &PredefinedMenuItem::copy(h, None)?,
            &PredefinedMenuItem::paste(h, None)?,
            &PredefinedMenuItem::select_all(h, None)?,
        ],
    )?;
    let view = Submenu::with_items(h, "View", true, &[&PredefinedMenuItem::fullscreen(h, None)?])?;
    let window = Submenu::with_id_and_items(
        h,
        tauri::menu::WINDOW_SUBMENU_ID,
        "Window",
        true,
        &[
            &PredefinedMenuItem::minimize(h, None)?,
            &PredefinedMenuItem::maximize(h, None)?,
            &PredefinedMenuItem::separator(h)?,
            &PredefinedMenuItem::close_window(h, None)?,
        ],
    )?;
    let help = Submenu::with_id_and_items(h, tauri::menu::HELP_SUBMENU_ID, "Help", true, &[])?;
    Menu::with_items(h, &[&app_menu, &file, &edit, &view, &window, &help])
}
