# Icon Composer 说明（0.2.0）

已用干净单层 SVG + `icon.json` 生成 `src-tauri/icons/AppIcon.icon`（白底 fill + Frame + KeyArrow，轻度 Liquid Glass）。
并用 `ictool --export-image` 导出了 Light/Dark 与小尺寸预览。

若还要在 GUI 里微调：

1. 打开：`open -a "Icon Composer" src-tauri/icons/AppIcon.icon`
   （完整路径：`/Applications/Xcode.app/Contents/Applications/Icon Composer.app`）
2. 左侧图层顺序自下而上应为：Frame → KeyArrow（白底已是 document fill，不必再加 rounded mask）。
3. 右侧 Appearance 切 Default / Dark，检查小尺寸预览。
4. Liquid Glass：Specular / Translucency 已偏轻（translucency 0.2）；可按观感再微调，**不要**烘焙圆角遮罩。
5. ⌘S 保存即可；Tauri `bundle.icon` 已包含 `icons/AppIcon.icon`，并保留 `icon.icns` 给旧系统回退。

预览图目录：`icon-src/logo03/composer-previews/`（已同步到 box `/workspace/clash-assist-icon-composer/`）。
