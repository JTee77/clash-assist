# Egress Checker

macOS **Apple Silicon (arm64)** 桌面应用：诊断代理**出口质量**——连通性、DNS / IPv6 / WebRTC 泄漏、出口 IP、Gemini / ChatGPT / 流媒体解锁、节点打分与快速测速。

面向已自备 **Clash Verge / Clash Verge Rev** 的用户。**当前仅支持 Clash Verge**，FlClash 的支持即将上线，会在充分实测之后再正式加入。**不提供任何节点或 VPN 服务**，也不是翻墙工具。仅支持 **Apple Silicon**，不承诺 Intel Mac。

许可证：**GPL-3.0**（GNU 通用公共许可证第 3 版）。

## 准备：启用 Clash Verge Rev / Mihomo API

1. 保持 **Clash Verge / Verge Rev** 运行（2.5.6+ 常用 Unix 服务套接字，可不开放 TCP external-controller）
2. 若使用较旧的 Clash Verge：可开启 **external-controller**（如 `127.0.0.1:9097`）并设置 **secret**
3. 确认 **mixed-port**（如 `7897`）已开启，供经代理的出口探针使用
4. 按需开启**系统代理**或 **TUN**

应用会读取 Verge 的 `config.yaml` / `clash-verge.yaml`，并自动探测控制器套接字（优先 `/var/run/clash-verge-service/users/<uid>/verge-mihomo.sock`，其次旧版 `/tmp/verge/verge-mihomo.sock`、`$TMPDIR/verge-mihomo.sock`）。即使 TCP external-controller 关闭或配置文件里的端口已过期，只要套接字可用仍能列出节点。也可在底部「高级」手动填 host / port / secret / mixed-port，或开 **Mock** 演示模式（无需真实 Clash）。

## 开发

依赖：Node.js 20+、Rust（stable）、Xcode Command Line Tools、pnpm。

```bash
git clone https://github.com/JTee77/egress-checker.git
cd egress-checker && pnpm install && pnpm tauri dev
```

**只用 `pnpm tauri dev`，且只开一个实例。** 别直接跑 `target/debug/egress-checker`（没有前端会白屏）。崩溃 / 白屏 / 进程残留时先 `pkill -f egress-checker && pkill -f vite`，再重开一个 dev。

| 命令 | 说明 |
|------|------|
| `pnpm tauri dev` | 开发模式（推荐） |
| `pnpm tauri dev -- --cli help --no-json` | CLI 帮助（隐藏窗） |
| `pnpm test` | 前端单测（vitest） |
| `pnpm typecheck` | TypeScript 检查 |
| `pnpm build` | 前端 typecheck + 生产构建 |
| `pnpm tauri build` | 打包 Mac `.app` / `.dmg`（需 Apple Silicon） |

> 非 macOS（如 Linux CI）上前端 `pnpm build` / `typecheck` 正常；原生 `tauri build` 面向 Mac，可能失败属预期。

## 功能

- **单页流程**（无侧栏多模式）：选软件 → 获取节点 → 环境检查 / 测节点；节点卡测过后可 **详情 + 再测**
- **主题**：浅色 / 深色 / 跟随系统
- **轻量门槛**：确认客户端连上、海外大致可通、是否像未走代理直连；不过则口语提示
- **测单个（完整）**：`runNodeDiagnostics` — 延迟、带宽、流媒体/商店/AI 等；节点卡「测 / 再测」走这条
- **测全部（完整深测）**：与测单个相同探针集（延迟淘汰后临时切换深测）；测完或中止会**强制切回**原节点
- **环境泄漏检查**：DNS / IPv6 / WebRTC / 分流 / 直连旁路（第二入口，不默认每次强跑）
- **CLI**：`--cli discover|nodes|gate|check|env|serve|install-shim --client verge`（目前仅支持 Clash Verge），stdout 为一行 CliEnvelope JSON（与 GUI 共用 runner），`--no-json` 为人类摘要；错误码与退出码见 [docs/CLI.md](./docs/CLI.md)，给 AI 助手的指南见 [docs/AGENTS-CLI.md](./docs/AGENTS-CLI.md)。二进制完整路径：`/Applications/Egress Checker.app/Contents/MacOS/egress-checker`；运行一次 `--cli install-shim` 后可直接用 `egress-checker`

## 打包（DMG）

Apple Silicon 上跑 `pnpm tauri build`，产物在 `src-tauri/target/release/bundle/dmg/*.dmg`。安装包带本地签名（ad-hoc，免费的完整性签名），但**未经苹果公证**。

### 首次打开说明

第一次打开时 macOS 会提示「Apple 无法验证…」，按下面三步操作，之后就能正常打开：

1. 弹窗里点 **「完成」**（不要点「移到废纸篓」）。
2. 打开 **系统设置 › 隐私与安全性**，拉到下方，点 **「仍要打开」**。
3. 再弹出的确认框里还是点 **「仍要打开」**，按提示完成身份验证。

## 免责声明

- 不提供节点、不运营 VPN
- 深度测速会产生一定流量，请自行注意费用
- DNS / WebRTC 检测在桌面 WebView 环境有诚实限制，结果为启发式，非实验室级证明

## 贡献 & License

欢迎提 Issue / PR，合入由维护者决定，详见 [CONTRIBUTING.md](./CONTRIBUTING.md)。

本项目采用 [GNU GPL-3.0](./LICENSE)（GNU 通用公共许可证第 3 版）：可以自由使用、修改和分发，包括商用；但分发修改后的版本时，必须同样以 GPL-3.0 公开源码。
