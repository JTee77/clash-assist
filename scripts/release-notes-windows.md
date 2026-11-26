## Clash伴侣 {{VERSION}}

<!-- clash-assist:platforms macos={{MACOS_VERSION}} windows={{WINDOWS_VERSION}} -->

### 更新

- 出口 IP 证据不足时标「未判定」，不再默认标成住宅，也不再因此加住宅分。只有组织名等明确命中机房关键词时，才标「疑似机房」。
- 规则模式下，优先策略组不含目标节点、且恰好只有另一个选择器含该节点时，改切那一组。订阅下发的节点也会并入切换校验，避免误报「切换失败」。

### Windows（x64，Windows 10 / 11）

- `Clash.Assist_{{VERSION}}_x64-setup.exe`：安装版，支持一键更新。
- `Clash.Assist_{{VERSION}}_x64-portable.zip`：便携版，解压即用，不支持一键更新。

Windows 与 Mac 都支持 Clash Verge Rev 和 FlClash。

### macOS

- `Clash.Assist_{{VERSION}}_aarch64.dmg`：仅支持 Apple Silicon（M 系列芯片）。

首次安装后打开若提示「无法验证」，到 **系统设置 › 隐私与安全性** 点 **「仍要打开」**。已安装旧版的用户直接点顶部 **「一键更新」** 即可。
