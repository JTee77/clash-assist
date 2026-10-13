# Clash Assist / Clash伴侣 CLI（0.2.0）

与图形界面共用 `src/lib/runner`、`egress`、`score`、`mihomo`。通过 **`--cli`** 启动隐藏窗口，在 WebView 内调用同一套 Tauri 能力。一次性命令结束后把 **一行 JSON（CliEnvelope）** 打到 stdout 并退出；**`serve`** 常驻本机 loopback HTTP，直到 Ctrl+C。

**目前只支持 Clash Verge / Clash Verge Rev（`--client verge`）。** FlClash 即将支持，暂不可用；其它软件不支持。

> 给 AI 助手 / 自动化脚本的简明指南见 [AGENTS-CLI.md](./AGENTS-CLI.md)；机器可读的命令清单：`egress-checker --cli help --json` 的 `data.spec`。

## 找到二进制 / 短命令

安装到「应用程序」后，二进制完整路径是：

```bash
"/Applications/Clash Assist.app/Contents/MacOS/egress-checker" --cli discover --client verge
```

想要短命令 `egress-checker`，运行一次：

```bash
"/Applications/Clash Assist.app/Contents/MacOS/egress-checker" --cli install-shim            # 自动选目录
"/Applications/Clash Assist.app/Contents/MacOS/egress-checker" --cli install-shim --dir ~/bin
egress-checker discover --client verge     # 之后即可（shim 已自动带 --cli）
```

- 写入 **一个** sh 包装脚本 `<dir>/egress-checker`，内容是 `exec '<本二进制完整路径>' --cli "$@"`（不是符号链接：避免 macOS 按链接路径找不到 .app 资源）。
- 目录：`--dir`（绝对路径或 `~/…`）优先；否则 `/usr/local/bin`（**存在且当前用户可写**时），否则 `~/.local/bin`（自动创建）。**不会** sudo / 提权。
- 目标已存在且**不是**本工具生成的 shim → `shim_exists`，不覆盖；是旧 shim 则原地更新（`created:false`）。
- 返回 `data`: `{ path, dir, target, created, onPath, targetIsDevBuild, appBinary, next, warning }`。`onPath:false` 时 `next` 给出要加进 `~/.zshrc` 的 `export PATH=…`。从开发构建（`target/debug|release`）运行时 `warning` 提醒：shim 会指向开发构建。
- 卸载：删掉该文件即可。应用移动位置后重新运行 install-shim。

## 用法

```bash
# 开发（会起 Vite；窗口应被隐藏）
pnpm tauri dev -- --cli help --no-json
pnpm tauri dev -- --cli discover --client verge
pnpm tauri dev -- --cli gate --client verge
pnpm tauri dev -- --cli check current --client verge
pnpm tauri dev -- --cli check node "节点名" --client verge
pnpm tauri dev -- --cli check all --client verge
pnpm tauri dev -- --cli env --client verge
pnpm tauri dev -- --cli serve --mock --port 17890

# 已编译二进制（debug 需要 Vite 在 127.0.0.1:1420；release 自带前端）
./src-tauri/target/debug/egress-checker --cli discover --mock
./src-tauri/target/release/egress-checker --cli help --no-json
```

### 命令

| 命令 | 含义 |
|------|------|
| `help` | 说明文本（`--no-json` 时直接打印；默认包在 `data.text`） |
| `discover` | 发现控制器并探测；返回状态、当前节点与节点摘要（最多 50 个） |
| `nodes` | **全部**节点（不截断），可 `--region` / `--match` / `--limit` 筛选 |
| `gate` | 轻量门槛（`runLightGate`），结果在 `data.gate` |
| `check current` | 当前出口 **完整**深测（不切换节点）。`check` 不带目标等同于它 |
| `check node <名>` | 指定节点完整深测（非当前节点时临时切换，测完切回） |
| `check <名>` | `check node <名>` 的简写（名不是 `current` / `node` / `all` 时） |
| `check all` | 全部节点 **完整**深测（逐个切换，结束切回；与 `check node` 相同探针集）。可加 `--region` / `--match` / `--limit N` 只测一部分 |
| `check nodes <名1> <名2> …` | 只测列出的节点（每个名字按 `check node` 同规则匹配；有一个找不到/有歧义就整体报错、不切换）。输出形状同 `check all`，切回保证相同；也可再叠加 `--region/--match/--limit` |
| `env` | 环境泄漏检查 |
| `serve` | 本机 **127.0.0.1** HTTP 服务（常驻到 Ctrl+C）。见下方「serve」 |
| `install-shim` | 安装短命令 `egress-checker`（见上方「找到二进制 / 短命令」） |

节点名匹配：先全名精确匹配，再唯一的「包含」匹配；匹配到多个报 `node_ambiguous`，找不到报 `node_not_found`。不会拿不存在的名字去切换。

### 选项

| 选项 | 说明 |
|------|------|
| `--client, -c <id>` / `--client=<id>` | 只接受 `verge`。`flclash` 等任何其它值都报 `client_unsupported`（「当前仅支持 Clash Verge」） |
| `--mock` | 不连真实软件，用演示节点跑通管线；可省略 `--client`（默认 verge）。**给了非法 `--client` 时 `--mock` 也照样报错，不回落** |
| `--port, -p <N>` / `--port=<N>` | **仅 `serve`**：监听端口。默认 **17890**；`0` = 临时端口（就绪横幅打印实际端口）。配其它命令 → `port_only_for_serve` |
| `--region <地区[,地区]>` | **仅 `nodes` / `check all` / `check nodes`**。按地区筛选：等于或包含节点 `region`（如 `日本`、`中国香港`），或别名（`JP`、`HK`、`香港`、`🇸🇬` …，与 GUI 分组同一规则 `detectRegion`）。逗号分隔表示「任一」 |
| `--match <词[,词]>` | 同上范围。节点名包含任一关键词（不区分大小写） |
| `--limit, -n <N>` | 同上范围。筛选后按列表顺序取前 N 个（正整数，否则 `invalid_limit`） |
| `--no-progress` / `--quiet` / `-q` | 不往 stderr 写 NDJSON 进度（见「进度事件」） |
| `--dir <路径>` | **仅 `install-shim`**：安装目录 |
| `--json` | 输出一行 CliEnvelope JSON（默认） |
| `--no-json` | 输出人类可读摘要：首行 `✓ <命令> 完成` 或 `✗ <命令> 失败 [code] message`，下面是要点（评分、节点列表、切回报错等）。脚本请用 JSON。`serve` 仍同时打 stderr 人类就绪文案 + stdout JSON 横幅 |
| `--help, -h` | 同 `help`（出现在任何位置都显示帮助） |
| `--` | 之后的词都当位置参数（节点名以 `-` 开头时用） |

解析是**严格**的：未知子命令、未知选项、选项缺值、多余参数、选项配错命令（`option_not_applicable`）都会 `ok:false` + 退出码 1，不会静默变成 `help`。取值选项都支持 `--opt v` 与 `--opt=v` 两种写法。
（不带 `--cli` 且第一个词也不是子命令时，进程按 GUI 启动——这条与 Rust `is_cli_mode` 同规则，两边共用用例 `src/lib/cli/cli-mode-cases.json`。）

> `--mock` 只替换「控制器 / 节点列表」：`gate`、`check`、`env` 的探针仍经本机代理口（默认 mixed-port 7897）真实发请求，`check` 含带宽抽样，会耗一定流量。`--mock` 下 `check node` / `check all` 不切换节点。

## CliEnvelope

```json
{
  "ok": true,
  "version": "0.2.0",
  "command": "discover",
  "ranAt": "2026-10-06T00:00:00.000Z",
  "data": { }
}
```

- 成功：`ok: true`，带 `data`。
- 失败：`ok: false`，带 `error: { code, message, hint }`。`hint` 是一句可执行的中文「下一步怎么做」，按 code 固定（全表见 `help --json` 的 `data.spec.errorCodes`）。部分失败（`restore_failed` / `aborted` / `check_failed` / `gate_failed`）**同时带 `data`**（已测到的结果、`restoreError` 等），便于脚本收尾。
- 未知命令时 `command` 回显用户写的词（如 `"frobnicate"`）。
- 密钥不会写入 JSON（`discover` 只回 host / port / mixedPort / source / sockPath）。
- **数字取整**：`data` 里所有非整数统一四舍五入到 **2 位小数**（Mbps、ms 等）；评分 `totalScore` 为 **1 位小数**。整数不变。说明文案字段（`blurb` / `conclusion` / `process` / `note` / `hint` 等）里 4 位以上的小数也取到 2 位；节点名等标识不动。

### 退出码

| 退出码 | 含义 |
|--------|------|
| `0` | 输出了 envelope，且 `ok: true` |
| `1` | 输出了 envelope，且 `ok: false`（含参数错误、闸门、门槛、检测失败、切回失败、中断） |
| `130` | 强制中断：第二次 Ctrl+C，或第一次中断后 90 秒仍未结束。**不输出 envelope，也不保证切回** |

### 错误码

| code | 何时 | 带 data |
|------|------|---------|
| `unknown_command` | 子命令不认识（`--cli frobnicate`） | 否 |
| `unknown_option` | 选项不认识（`--verbose`、`-x`） | 否 |
| `missing_option_value` | `--client` / `-c` / `--client=` 没给值 | 否 |
| `unexpected_argument` | 子命令带了多余参数（`discover foo`、`check all x`、`help check`） | 否 |
| `node_name_required` | `check node` 没给名字 | 否 |
| `client_required` | 没给 `--client` 且没加 `--mock` | 否 |
| `client_unsupported` | `--client` 不是 `verge`（含 `flclash`；`--mock` 也一样） | 否 |
| `option_not_applicable` | 选项配错命令（如 `discover --limit 2`、`check node X --region JP`、非 install-shim 带 `--dir`） | 否 |
| `invalid_limit` | `--limit` 不是正整数 | 否 |
| `nodes_unavailable` | `check node` / `check nodes` / 筛选时读不到节点列表 | 否 |
| `no_nodes_matched` | `check all` 的筛选条件一个节点都没匹配到（不切换、不测） | 否 |
| `os_unsupported` | macOS 低于 14 时运行 `check` / `env`（隐藏窗会被系统挂起导致卡死，故直接拒绝） | 否 |
| `node_not_found` | `check node` 找不到该名字 | 否 |
| `node_ambiguous` | `check node` 的名字匹配到多个节点 | 否 |
| `gate_failed` | `check node` / `check all` 的轻量门槛没过（没切换、没深测） | `{ target, clientId, gate }` |
| `check_failed` | 检测没完成（切换失败、找不到策略组、没有节点可测、过程异常） | 对应 check 形状 |
| `restore_failed` | 临时切换后**没能切回原节点**（最高优先级，请立刻到代理软件里手动选回） | 对应 check 形状 |
| `aborted` | 收到 Ctrl+C / SIGTERM / SIGHUP，已停止（`check node`/`all` 已按需切回） | check 形状；其它命令无 |
| `dispatch_error` | 调度过程中抛出的未预期异常 | 否 |
| `boot_error` / `argv_unavailable` | CLI 启动层异常 | 否 |
| `invalid_port` | `--port` 不是 0–65535 整数 | 否 |
| `port_only_for_serve` | `--port` 配了非 serve 命令 | 否 |
| `serve_bind_failed` | serve 无法绑定端口 | 否 |
| `serve_not_oneshot` | 误把 serve 当一次性 dispatch | 否 |
| `serve_handler_error` | serve 处理单个请求时抛异常 | 否 |
| `unknown_route` | serve 内部路由不认识（Rust/前端版本不一致） | 否 |
| `invalid_body` | POST /v1/check/node、/v1/nodes、/v1/check/all 的 JSON 不合法或字段类型不对 | 否 |
| `shim_exists` | install-shim 目标已有同名文件且不是本工具生成 | 否 |
| `shim_failed` | install-shim 无法创建目录/写文件，或 `--dir` 不是绝对路径 | 否 |
| `shim_unavailable` | 不在应用二进制里运行 install-shim | 否 |
| `unauthorized` / `not_found` / `method_not_allowed` / `timeout` / `server_stopping` | serve 的 HTTP 层错误（Rust 直接回，同样带 `hint`） | 否 |

同时满足多条时的优先级：`restore_failed` > `aborted` > `check_failed`。

注意：`discover` 连不上软件、`gate` 门槛不过时仍是 `ok: true`——这两个命令本身就是「报告状态」，请看 `data.status` / `data.gate.ok`。

## data 形状

### discover

```jsonc
{
  "clientId": "verge",
  "status": "connected",        // unknown | connected | unreachable | unauthorized | mock
  "message": "…",
  "currentProxy": "香港 01",
  "currentProxyReason": null,   // currentProxy 为 null 时的中文原因（未连上 / 读节点失败 / 主选择组在 DIRECT 或无法唯一判定）
  "usingMock": false,
  "config": { "host": "127.0.0.1", "port": 9097, "mixedPort": 7897, "source": "…", "sockPath": null },
  "nodeCount": 80,              // 总数
  "nodesTruncated": true,       // nodeCount > 50
  "nodesError": null,           // 已连上但读节点列表失败时的原因
  "nodes": [ { "name": "…", "type": "…", "region": "…" } ]  // 最多 50 个
}
```

`currentProxy` 与 GUI 同一判定（`src/lib/mihomo/currentProxy.ts` / Rust `nodes.rs::resolve_current_proxy`）：沿主选择组（Proxy / GLOBAL / 「🚀 节点选择」…）的 `now` 链走到真实节点。**规则模式**下 GLOBAL 常停在 DIRECT，会跳过它去看主选择组。（0.1.16 及以前 CLI 只用了 discover 阶段的值，规则模式下恒为 `null`；0.1.17 修复。）

### nodes

```jsonc
{
  "clientId": "verge",
  "status": "connected",
  "currentProxy": "香港 01",
  "total": 80,                  // 筛选前总数
  "count": 12,                  // 本次返回数
  "filter": { "region": "日本", "match": null, "limit": null },
  "nodesError": null,
  "nodes": [ { "name": "日本 01", "type": "vless", "region": "日本", "current": false, "lastDelay": 132 } ]
  // lastDelay：Clash Verge 最近一次测速（ms）；0 = 失败；null = 没测过
}
```

### gate

```jsonc
{ "clientId": "verge", "connectionStatus": "connected", "gate": { "ok": true, "message": "…", "process": "…" } }
```

### env

```jsonc
{ "clientId": "verge", "cards": [ /* CheckCard[] */ ] }
```

### check —— 三种形状（按 `data.target` 区分）

**`target: "current"`**（不切换节点，无门槛前置）

```jsonc
{
  "target": "current",
  "clientId": "verge",
  "nodeName": "香港 01",        // 读不到时为 "当前节点"
  "score": { "stars": 4.5, "totalScore": 86, "blurb": "…" },
  "cards": [ /* CheckCard[] */ ],
  "note": "…"
}
```

**`target: "node"`**

```jsonc
{
  "target": "node",
  "clientId": "verge",
  "nodeName": "香港 01",
  "score": { "stars": 4, "totalScore": 80, "blurb": "…" },   // 没测成为 null
  "cards": [ /* CheckCard[] */ ],
  "hint": null,                 // 过程提示（找不到策略组、切换失败…）
  "restoreError": null,         // 没能切回时的报错；与 hint 分开
  "aborted": false,
  "gate": { "ok": true, "message": "…" }
}
```

**`target: "all"`**

```jsonc
{
  "target": "all",
  "clientId": "verge",
  "count": 12,
  // 加了筛选或用 check nodes 时：本次实际纳入的节点；未筛选为 null
  "selection": { "filter": { "region": "HK", "match": null, "limit": 2 }, "names": null, "selected": ["香港 01", "香港 02"] },
  "scores": [ { "nodeName": "…", "stars": 4.5, "totalScore": 86, "blurb": "…" } ],  // 按星级、总分降序
  "hint": null,
  "restoreError": null,
  "aborted": false,             // true 时 scores 只含已测完的节点
  "nodesError": null,
  "gate": { "ok": true, "message": "…" }
}
```

`count` / `scores` 含预检判死的节点（`stars:"unavailable"`）。`--limit` 限的是「纳入」的节点数，不是成功数。

`stars` 取值 `1 | 1.5 | … | 5 | "unavailable"`。`restoreError` 文案与 GUI 红字同一句（`src/lib/runner/restore.ts`）：`没能切回原先节点「X」。`，或没记下原节点时 `没能切回原先节点。`。

### CheckCard

`check` 的 `cards` 与 `env` 的 `cards` 都是 `CheckCard[]`（`src/lib/egress/types.ts`）：

| 字段 | 类型 | 说明 |
|------|------|------|
| `id` | string | 稳定标识，见下 |
| `title` | string | 中文标题（展示用，可能改文案；脚本请用 `id`） |
| `level` | `"pass" \| "warn" \| "fail" \| "unknown" \| "running"` | 结果等级。`unknown` = 没测出结论；`running` 只会出现在中断/异常留下的残卡里 |
| `conclusion` | string | 一句话中文结论 |
| `process` | string? | 技术细节（URL、HTTP 码、耗时） |
| `suggestion` | string? | 下一步建议 |
| `unverified` | boolean? | true = 没法经代理验证（如没有 mixed-port），评分时跳过该维度 |
| `metrics` | object? | 机器可读数值：`downMbps` / `upMbps` / `latencyMs` / `exitIp` / `countryCode` / `hosting` / `unlockSupported` / `unlockLevel`（`full\|web_only\|app_only\|blocked\|unknown`）/ `unlockRegion` |
| `summary` / `detail` / `tip` | string? | 已废弃的旧字段，可能出现，忽略即可 |

`id` 取值：

- 节点深测（`check`）：`reachability` 连通性、`exit-ip` 出口 IP、`latency` 延迟采样、`bandwidth` 抽样带宽、`gemini`、`chatgpt`、`netflix`、`disney`、`youtube`、`tiktok`、`spotify`、`prime-video`、`app-store`、`google-play`
- 环境（`env`）：`bare-egress` 直连旁路、`dns-leak` DNS 解析器、`ipv6-leak` IPv6 泄漏、`webrtc`、`split-routing` 分流

## 进度事件（stderr NDJSON）

`check`（current / node / nodes / all）与 `env` 运行时，**stderr 每行一个 JSON 对象**；stdout 仍只有最后一行 envelope。`--no-progress` / `--quiet` 关闭。`discover` / `nodes` / `gate` / `help` 不发。serve 不经 HTTP 推送进度。

```jsonc
{"type":"start","command":"check","ts":"…","text":"check all"}
{"type":"stage","command":"check","ts":"…","stage":"connect","text":"连接 Clash Verge、读取节点"}
{"type":"stage","command":"check","ts":"…","stage":"gate","text":"轻量门槛"}
{"type":"stage","command":"check","ts":"…","stage":"precheck","index":3,"total":5,"text":"连通性预检 3/5"}
{"type":"stage","command":"check","ts":"…","stage":"test","index":1,"total":2,"nodeName":"香港 01","text":"检测 1/2"}
{"type":"card","command":"check","ts":"…","cardId":"latency","level":"pass","nodeName":"香港 01"}
{"type":"node_done","command":"check","ts":"…","nodeName":"香港 01","index":1,"total":2,"stars":4.5,"totalScore":86.3}
{"type":"stage","command":"check","ts":"…","stage":"restore","text":"正在切回原先节点：香港 01…"}
{"type":"done","command":"check","ts":"…","ok":true}
```

- `type`：`start` | `stage` | `card` | `node_done` | `done`。
- `stage`：`connect` → `gate`（node/all）→ `precheck`（all：并发测延迟，`index` 为已完成数）→ `switch`（node）→ `test` → `restore`（切换过才有）；`env` 为 `probe`。
- `index` 1 起；`total` 为本阶段总数。预检判死的节点也会发 `node_done`（无 `index`，`stars:"unavailable"`）。
- 解析建议：逐行读 stderr，**只处理以 `{` 开头且能 JSON.parse 的行**（中断提示等人类文案也写 stderr）。

## 中断（Ctrl+C）

直接运行二进制时：

1. **第一次** Ctrl+C / SIGTERM / SIGHUP：Rust 捕获信号，只置位；stderr 提示「正在停止」。前端每 250ms 轮询 `cli_abort_requested`：
   - `check all`：与 GUI「停止并切回」同一机制——**当前节点测完后**停止（不打断正在跑的深测），切回原节点，输出 `aborted`（或切回失败时 `restore_failed`）envelope，退出码 1。
   - `check node`：单节点深测本身不可中途打断；测完照常切回，再输出 `aborted` envelope。
   - `discover` / `gate` / `env` / `check current`：不切换节点，立即输出 `aborted` envelope 并退出 1。
2. **看门狗**：第一次信号后 90 秒仍未退出 → 强制退出 130（不保证切回）。
3. **第二次** Ctrl+C：立即退出 130，不切回；stderr 提醒到代理软件里确认当前节点。

已知限制：

- 正在进行的单节点深测（含带宽抽样）不会被中途取消，最坏要等它跑完（通常几十秒）才开始切回。
- SIGKILL（`kill -9`）、断电等无法捕获，不会切回。
- 经 `pnpm tauri dev` 运行时，Ctrl+C 也会发给 pnpm / cargo 等父进程，它们可能先把应用杀掉；需要可靠中断请直接运行编译好的二进制。

## 自检

```bash
pnpm exec vitest run src/lib/cli src/lib/runner   # 单测（参数、闸门、切回、中断、serve 路由、TS/Rust 同步）
pnpm exec vitest run src/lib/tauriInvokeContract.test.ts  # 所有 invoke(...) 参数键 == Rust 命令参数名（如 cli_serve_respond 需 { req }）
(cd src-tauri && cargo test cli_serve -- --nocapture)  # serve：鉴权 / 路由 / 只绑 127.0.0.1
node scripts/simulate-restore-fail.mjs            # 切回文案（直接 import runner 源码；Node ≥ 22.18）
bash scripts/cli-smoke.sh                         # 上面两项
EGRESS_CLI_SMOKE=1 bash scripts/cli-smoke.sh      # 再跑真实二进制：help / 报错退出码 / discover / gate / check current / serve（--mock：/health、POST /v1/discover×2、401）
```

## 已知限制

- 需要 macOS 上的应用二进制（或 `pnpm tauri dev`）；**不是**独立的纯 Node CLI。
- 隐藏窗需关闭 WebView 后台节流才能跑深测（否则 WKWebView 会挂起不在屏幕上的页面，`check` 永远不返回——0.1.15 及以前即如此）。0.1.16 起 CLI 模式下主窗以「隐藏 + 不节流」创建；该开关需 **macOS 14+**。0.1.17 起更早的系统上 `check` / `env` **立即**返回 `os_unsupported`（不再卡住）；`discover` / `nodes` / `gate` 不受影响。GUI 模式不受影响。
- `check all` 会切换节点并耗时/耗流量，请谨慎。
## serve（localhost HTTP）

```bash
egress-checker --cli serve --client verge [--port 17890]
egress-checker --cli serve --mock --port 0          # 临时端口，看就绪横幅
```

- **只绑定 `127.0.0.1`**（绝不 `0.0.0.0`）。默认端口 **17890**；`--port 0` 由内核分配，就绪时打印实际端口。
- **鉴权**：启动时生成一次性随机 token，打印到 **stderr（人类文案）** 与 **stdout（CliEnvelope `command:"serve"` 的 `data.token`）**。之后每个请求必须带：
  - `Authorization: Bearer <token>`，或
  - `X-Egress-Token: <token>`
  - 缺/错 → **401** `{ok:false,error:{code:"unauthorized",…}}`
  - **包括 `GET /health`**：没有匿名健康检查（与其它路由一致，避免本机其它进程探测/枚举）。
- **路由**（业务结果尽量为 HTTP 200 + CliEnvelope JSON；鉴权/路径错误用 401/404/405）：

| 方法 | 路径 | 说明 |
|------|------|------|
| `GET` | `/health` | `{ok:true}`（Rust 直接回，不经 WebView）。**同样需要 token**，无 token → 401 |
| `POST` | `/v1/discover` | 同 `--cli discover` |
| `POST` | `/v1/nodes` | 同 `--cli nodes`；可选 body：`{"region":"日本","match":"IPLC","limit":10}`（region/match 也可为字符串数组） |
| `POST` | `/v1/gate` | 同 `--cli gate` |
| `POST` | `/v1/env` | 同 `--cli env` |
| `POST` | `/v1/check/current` | 同 `--cli check current` |
| `POST` | `/v1/check/all` | 同 `--cli check all`；可选 body：`{"region":…,"match":…,"limit":2,"names":["香港 01","日本 02"]}`（`names` 等价 `check nodes`）。空 body = 全部。未知字段 → `invalid_body` |
| `POST` | `/v1/check/node` | body：`{"name":"节点名"}`，同 `--cli check node` |

- 会话级 `--client` / `--mock` 来自启动 argv（与一次性 CLI 同一闸门：仅 `verge`）。
- **架构**：隐藏窗 + WebView 跑原来的 `dispatchCli`；Rust 用 std TCP 做 loopback HTTP，鉴权通过后把任务丢进通道，前端 `cli_serve_poll` → `dispatchServeJob` → `cli_serve_respond`。**请求串行**（同一时间只处理一个 WebView 任务；并发连接会排队）。
- **Ctrl+C**：停止监听，尽量让进行中的 `check node/all` 切回后结束，然后进程退出 0。再按一次 / 超时仍走一次性 CLI 的 130 看门狗。
- **不是**通用 Web API：无 WebSocket、不为浏览器开 CORS、不提供 GUI。

示例：

```bash
# 终端 A
./egress-checker --cli serve --mock --port 17890
# 记下 token

# 终端 B
TOKEN=…
curl -s -H "Authorization: Bearer $TOKEN" http://127.0.0.1:17890/health
curl -s -H "X-Egress-Token: $TOKEN" -X POST http://127.0.0.1:17890/v1/discover
curl -s -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json'   -d '{"name":"香港"}' -X POST http://127.0.0.1:17890/v1/check/node
```
