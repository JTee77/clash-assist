fn main() {
    // windows-plan §2.7 / V11：Windows 可执行文件内嵌的应用清单在 tauri-build 默认清单
    // （Common Controls v6）之外再声明 Per-Monitor v2 DPI 感知；CI 用 mt.exe 导出校验。
    // 非 Windows 目标忽略此项。其余行为同 `tauri_build::build()`。
    println!("cargo:rerun-if-changed=windows/app.manifest");
    let windows = tauri_build::WindowsAttributes::new()
        .app_manifest(include_str!("windows/app.manifest"));
    let attrs = tauri_build::Attributes::new().windows_attributes(windows);
    if let Err(error) = tauri_build::try_build(attrs) {
        println!("{error:#}");
        std::process::exit(1);
    }
}
