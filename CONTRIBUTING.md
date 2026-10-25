# 贡献指南

欢迎给 [Clash伴侣 / Clash Assist](https://github.com/JTee77/clash-assist) 提建议和补丁。

## 怎么参与

1. 用 **Issue** 反馈问题、想法或讨论较大改动（非小修小补时建议先开 Issue）。
2. **Fork** 本仓库，在你自己的副本上新建分支修改。
3. 提交 **Pull Request**。是否合入由维护者决定；请不要期待能直接推送到 `main`（`main` 已受保护）。

## 合入原则

- 小而清晰的改动更容易被接受。
- 文案与界面请保持简体中文、少黑话，面向普通用户。
- 合入前维护者可能要求调整或补充说明。

## 许可

本项目采用 [GNU GPL-3.0](./LICENSE)（GNU 通用公共许可证第 3 版）：可以自由使用、修改和分发；分发修改后的版本时，必须同样以 GPL-3.0 公开源码。贡献即表示你同意你的贡献可按同一许可证收录进本项目。


---

# 开发者说明（从 README 移来）

### 准备：启用 Clash Verge Rev / Mihomo API

1. 保持 **Clash Verge / Verge Rev** 运行（2.5.6+ 常用 Unix 服务套接字，可不开放 TCP external-controller）
2. 若使用较旧的 Clash Verge：可开启 **external-controller**（如 `127.0.0.1:9097`）并设置 **secret**
3. 确认 **mixed-port**（如 `7897`）已开启，供经代理的出口探针使用
4. 按需开启**系统代理**或 **TUN**

应用会读取 Verge 的 `config.yaml` / `clash-verge.yaml`，并自动探测控制器套接字（优先 `/var/run/clash-verge-service/users/<uid>/verge-mihomo.sock`，其次旧版 `/tmp/verge/verge-mihomo.sock`、`$TMPDIR/verge-mihomo.sock`）。即使 TCP external-controller 关闭或配置文件里的端口已过期，只要套接字可用仍能列出节点。也可在底部「高级」手动填 host / port / secret / mixed-port，或开 **Mock** 演示模式（无需真实 Clash）。

### 开发

依赖：Node.js 20+、Rust（stable）、Xcode Command Line Tools、pnpm。

```bash
git clone https://github.com/JTee77/clash-assist.git
cd clash-assist && pnpm install && pnpm tauri dev
```

**只用 `pnpm tauri dev`，且只开一个实例。** 别直接跑 `target/debug/clash-assist`（没有前端会白屏）。崩溃 / 白屏 / 进程残留时先 `pkill -f clash-assist && pkill -f vite`，再重开一个 dev。

| 命令 | 说明 |
|------|------|
| `pnpm tauri dev` | 开发模式（推荐） |
| `pnpm tauri dev -- --cli help --no-json` | CLI 帮助（隐藏窗） |
| `pnpm test` | 前端单测（vitest） |
| `pnpm typecheck` | TypeScript 检查 |
| `pnpm build` | 前端 typecheck + 生产构建 |
| `pnpm tauri build` | 打包 Mac `.app` / `.dmg`（需 Apple Silicon） |

> 非 macOS（如 Linux CI）上前端 `pnpm build` / `typecheck` 正常；原生 `tauri build` 面向 Mac，可能失败属预期。

### 打包（DMG）

Apple Silicon 上跑 `pnpm tauri build`，产物在 `src-tauri/target/release/bundle/dmg/*.dmg`。安装包带本地签名（ad-hoc，免费的完整性签名），但**未经苹果公证**。

DMG 窗口 540×360、图标 96。背景源图：`src-tauri/dmg/dmg-background.png`（540×360）与 `dmg-background@2x.png`（1080×720），替换后运行 `scripts/make-dmg-background.sh` 重新生成 `background.tiff`。

0.2.0 起 bundle identifier 改为 `com.clashassist.app`，短命令改为 `clash-assist`；首次启动会把旧 `com.egresschecker.app` 的数据 / WebKit 设置复制过来（旧目录保留），`install-shim` 会删除旧版生成的 `egress-checker` 短命令。

### 功能

- **单页流程**（无侧栏多模式）：选软件 → 刷新节点 → 环境检查 / 测节点；节点卡测过后可 **详情 + 再测**
- **主题**：浅色 / 深色 / 跟随系统
- **轻量门槛**：确认客户端连上、海外大致可通、是否像未走代理直连；不过则口语提示
- **测单个（完整）**：`runNodeDiagnostics` — 延迟、带宽、流媒体/商店/AI 等；节点卡「测 / 再测」走这条
- **测全部（完整深测）**：与测单个相同探针集（延迟淘汰后临时切换深测）；测完或中止会**强制切回**原节点
- **环境泄漏检查**：DNS / IPv6 / WebRTC / 分流 / 直连旁路（第二入口，不默认每次强跑）
- **CLI**：`--cli discover|nodes|gate|check|env|serve|install-shim --client verge`（目前仅支持 Clash Verge），stdout 为一行 CliEnvelope JSON（与 GUI 共用 runner），`--no-json` 为人类摘要；错误码与退出码见 [docs/CLI.md](./docs/CLI.md)，给 AI 助手的指南见 [docs/AGENTS-CLI.md](./docs/AGENTS-CLI.md)。二进制完整路径：`/Applications/Clash Assist.app/Contents/MacOS/clash-assist`；运行一次 `--cli install-shim` 后可直接用 `clash-assist`
