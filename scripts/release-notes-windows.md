## Clash伴侣 {{VERSION}}

### Windows（x64，Windows 10 / 11）

- `Clash.Assist_{{VERSION}}_x64-setup.exe`：安装版，按当前用户安装，不需要管理员权限；支持应用内一键更新。
- `Clash.Assist_{{VERSION}}_x64-portable.zip`：便携版，解压后直接运行 `clash-assist.exe`；不提供一键更新，新版本请到发布页下载。便携版使用系统自带的 WebView2 运行时（Windows 10 1803 及以后版本、Windows 11 均已自带）。
- `SHA256SUMS.txt`：以上文件的 SHA-256 校验值。

Windows 版目前只支持 Clash Verge Rev。

可以用 GitHub CLI 核对 Windows 文件的构建来源：

```
gh attestation verify Clash.Assist_{{VERSION}}_x64-setup.exe -R {{REPO}}
```

### macOS

（发布人上传 DMG 后在此补充。）
