# Clash伴侣 / Clash Assist — 产品说明（v0.2.1）

## 定位
**Clash伴侣**（Clash Assist）是 macOS（Apple Silicon）桌面应用，诊断代理**出口质量**——不只是「能不能打开国外网站」。

一句话：**看看你的代理会不会漏 DNS、关心的服务能不能用、实际够不够快。**

**不提供**任何节点或 VPN 服务。当前仅支持 Clash Verge / Clash Verge Rev。FlClash 的支持即将上线，充分实测后再正式加入。

许可证：**GPL-3.0**（GNU 通用公共许可证第 3 版；可商用，分发改版须同样开源）。

## 受众
- 已在用 Clash Verge / Clash Verge Rev 的 Mac 用户（FlClash 即将支持，尚未正式加入）
- 需要比「能开 Google」更深的检查
- 普通用户可直接用 GUI；进阶用户可用 CLI 拿 JSON

## 平台
- **仅 macOS Apple Silicon（arm64）**（v1 不做 Intel）
- Windows / 移动端不在当前范围
- 分发：GitHub Releases（`.dmg` / `.app`），不强制上架 App Store

## 架构（两层）
1. **通用出口诊断** — 测当前系统出口（系统代理 / TUN 开启时）
2. **Mihomo 适配** — 本地 `external-controller` / secret / Unix 套接字：节点列表、延迟、切换、深测

文案统一称：**「Mihomo / Clash Meta 兼容客户端」**。

## 明确不做
- Shadowrocket / Surge / Quantumult / 封闭商业 VPN 一键适配
- 提供或售卖节点 / 订阅
- 账号体系、云同步、可识别用户的遥测
- 对无开放 API 的客户端假装「全节点扫遍」
- 宣称「翻墙 / 突破防火墙」

## 当前版本与本轮目标（0.2.0）

**当前版本：0.2.1**（产品名 Clash伴侣 / Clash Assist）。

本轮目标：**CLI 补齐（面向 AI 助手）**——不加与 CLI 无关的功能：
- `install-shim` 短命令（`clash-assist`），文档写明应用内完整路径
- `check` / `env` 往 stderr 写 NDJSON 进度（`--no-progress` 关闭），stdout 仍是一行 envelope
- `nodes` 全量节点列表（`--region` / `--match` / `--limit`）；`discover` 行为不变
- 子集检测：`check all --region/--match/--limit`、`check nodes <名…>`；serve `/v1/check/all`、`/v1/nodes` 接受同样的 body
- 规则模式下 `currentProxy` 与 GUI 同一判定（修复 CLI 恒为 null），识别不到时给 `currentProxyReason`
- 输出数字取整；macOS < 14 跑 `check` / `env` 立即 `os_unsupported`；每个错误码带中文 `hint`
- `help --json` 的 `data.spec` 机器可读清单；新增 [docs/AGENTS-CLI.md](./docs/AGENTS-CLI.md)
- 合入 ad-hoc 签名 + DMG 背景（[PR #23](https://github.com/JTee77/clash-assist/pull/23)）

### 0.1.16

目标：**CLI 收口**（闸门、严格解析、切回、Ctrl+C、文档）+ **最小可用 `serve`**（127.0.0.1 + token 鉴权，映射 discover/gate/env/check）。许可证已改为 **GPL-3.0**（[PR #20](https://github.com/JTee77/clash-assist/pull/20)）。CLI 收口见 [PR #21](https://github.com/JTee77/clash-assist/pull/21)；`serve` 见 [PR #22](https://github.com/JTee77/clash-assist/pull/22)。

0.1.15 已发：窄窗排版、使用中识别、FlClash 即将支持、商店只写通/不通、取消记录过期清理等。

图标本版不动。

「明确不做」见上文，本版不变。

## 当前产品形态（0.1.12 基线 → 0.1.17）

### 单页 GUI（无侧栏多模式）
- **无**「首页 | 节点 | 设置 | 关于」侧栏多页；工作区是一条渐进流程：选软件 → 获取节点 → 环境检查 / 测节点
- 节点瀑布流卡片；测过后可 **详情 + 再测** 并存
- 主题：浅色 / 深色 / 跟随系统（右上角切换）

### 检测分工（已定）
| 动作 | 实现 | 说明 |
|------|------|------|
| 测单个（含「再测」） | `runNodeDiagnostics` | **完整**深测（延迟、full 带宽、流媒体/商店/AI 等） |
| 测全部 | `runNodeDiagnostics` | **完整**深测（与测单个相同：延迟、full 带宽、流媒体/商店/AI 等） |
| 环境泄漏 | `runEnv` / `runEnvDiagnostics` | DNS / IPv6 / WebRTC / 分流 / 直连旁路；第二入口，不默认每次强跑 |

前端业务调度在 **`src/lib/runner`**（无 React）；GUI 只接线。评分读 **`CheckCard.metrics`**（不再文案反解 Mbps）。

### 轻量门槛
获取节点后先 `runLightGate`：客户端是否连上、海外是否大致通、是否像未走代理直连。不过则口语提示，不进入节点测评。

### CLI（0.1.12 骨架；0.1.16 收口 + 最小 serve；0.1.17 面向 AI 助手补齐）
与 GUI **共用** `lib/runner` + `egress` + `score` + `mihomo`。**目前只支持 `--client verge`**（Clash Verge / Clash Verge Rev）；FlClash 即将支持，暂不可用。

```text
clash-assist --cli discover --client verge
clash-assist --cli nodes --client verge [--region 日本] [--match 关键词] [--limit N]
clash-assist --cli gate --client verge
clash-assist --cli check current|node <名>|all --client verge
clash-assist --cli check all --client verge --region HK --limit 3
clash-assist --cli check nodes "香港 01" "日本 02" --client verge
clash-assist --cli help --json        # data.spec 机器可读
clash-assist --cli install-shim       # 之后可直接 clash-assist …
clash-assist --cli env --client verge
clash-assist --cli check all --mock --no-json
clash-assist --cli serve --client verge [--port 17890]
clash-assist --cli serve --mock
```

- 一次性命令：stdout 为一行 **CliEnvelope** JSON（`ok/version/command/ranAt/data|error`）；`--no-json` 输出人类摘要。
- **`serve`**：只绑 `127.0.0.1`（默认端口 17890）；启动打印一次性 token；请求需 `Authorization: Bearer` 或 `X-Egress-Token`；路由映射到上述命令（`GET /health` + `POST /v1/…`）。隐藏窗 + WebView 调度，**请求串行**。Ctrl+C 停服。不是通用 Web API（无 CORS / WebSocket / 浏览器 GUI）。
- 退出码：`0` = ok:true（serve 正常停服亦为 0），`1` = ok:false；强制中断（第二次 Ctrl+C / 等待切回超时）为 `130`。
- 严格解析：未知命令 / 未知选项 / 多余参数 / 非法 `--client` 都是 ok:false，不静默变成 help、不回落。
- `check node` / `check all` 切回失败 → `restore_failed`，`data.restoreError` 与 GUI 红字同一句。
- Ctrl+C：`check all` 在当前节点测完后停止并切回（同 GUI「停止并切回」）。

- 进度：`check` / `env` 往 stderr 写 NDJSON（`--no-progress` 关闭）；错误带中文 `hint`；数字取整；macOS < 14 的 `check` / `env` 直接 `os_unsupported`。

错误码、三种 `check` 数据形状、`discover` 节点截断、serve 鉴权与路由等细节见 [docs/CLI.md](./docs/CLI.md)。

## 非目标回顾（仍成立）
见上文「明确不做」。深度测速可能耗流量；DNS/WebRTC 在桌面 WebView 下为启发式结论。
