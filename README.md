# Clash伴侣 / Clash Assist

**Clash伴侣**（英文名 Clash Assist）是 macOS Apple Silicon 桌面应用，用来检查 Clash Verge 代理节点的出口质量：连通性、DNS / IPv6 / WebRTC 泄漏、出口 IP、AI 与流媒体解锁、测速。

目前仅支持 Clash Verge，不提供任何节点或 VPN 服务。

## 安装

从 [Releases](https://github.com/JTee77/clash-assist/releases) 下载 DMG，把 Clash伴侣 拖进「应用程序」。

### 首次打开说明

第一次打开时 macOS 会提示「Apple 无法验证…」，按下面三步操作，之后就能正常打开：

1. 弹窗里点 **「完成」**（不要点「移到废纸篓」）。
2. 打开 **系统设置 › 隐私与安全性**，拉到下方，点 **「仍要打开」**。
3. 再弹出的确认框里还是点 **「仍要打开」**，按提示完成身份验证。

### 「本地网络」权限说明

「环境泄漏检查」中的 DNS 和 WebRTC 两项需要 macOS「本地网络」权限。

## 命令行

运行一次 `/Applications/Clash\ Assist.app/Contents/MacOS/clash-assist --cli install-shim` 后，可直接使用 `clash-assist --cli help`。详见 [docs/CLI.md](./docs/CLI.md)。

## 免责声明

- 不提供节点、不运营 VPN
- 深度测速会产生一定流量，请自行注意费用
- DNS / WebRTC 检测结果为启发式参考

## 贡献 & License

欢迎提 Issue / PR，开发说明见 [CONTRIBUTING.md](./CONTRIBUTING.md)。本项目采用 [GNU GPL-3.0](./LICENSE)。
