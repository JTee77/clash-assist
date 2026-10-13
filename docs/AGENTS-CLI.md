# Clash Assist / Clash伴侣 CLI —— 给 AI 助手的使用指南（0.2.0）

完整参考见 [CLI.md](./CLI.md)。本页只讲「怎么稳妥地调用」。

## 1. 找到二进制

按顺序尝试：

1. `command -v clash-assist`（用户运行过 `install-shim` 后存在；shim 自带 `--cli`，再加一次 `--cli` 也没关系）
2. `"/Applications/Clash Assist.app/Contents/MacOS/clash-assist" --cli …`
3. 开发仓库里：`src-tauri/target/release/clash-assist --cli …`（debug 构建需要 Vite 在 127.0.0.1:1420）

没有 shim 时可以建议用户运行一次（会写文件到 PATH 目录，先征得同意）：

```bash
"/Applications/Clash Assist.app/Contents/MacOS/clash-assist" --cli install-shim
```

只支持 macOS、只支持 Clash Verge（`--client verge`）。`check` / `env` 需要 **macOS 14+**，更早系统返回 `os_unsupported`。

## 2. 先读机器可读清单

```bash
clash-assist --cli help --json   # data.spec：commands / options / errorCodes(+hint) / 建议超时
```

## 3. 推荐调用顺序

```text
discover  →  gate  →  nodes（选节点） →  check current | check node <名> | check nodes <名…> | check all --limit N
```

| 步骤 | 目的 | 看什么 |
|------|------|--------|
| `discover --client verge` | 是否连上、当前节点 | `data.status == "connected"`；`data.currentProxy`（null 时看 `currentProxyReason`） |
| `gate --client verge` | 是否真的走代理 | `data.gate.ok`；为 false 就停下，把 `data.gate.message` 告诉用户 |
| `nodes --client verge [--region 日本] [--match 关键词]` | 拿准确节点名 | `data.nodes[].name`（不截断）；`current:true` 是在用节点 |
| `check …` | 深测 | `data.score` / `data.scores`、`data.cards`；**务必检查 `data.restoreError`** |

注意 `discover` 连不上、`gate` 不过时仍是 `ok:true`（它们是「报告状态」的命令）。

## 4. 输出与错误

- **stdout 只有一行 JSON**：`{ ok, version, command, ranAt, data?, error? }`。退出码 0 = ok:true，1 = ok:false，130 = 被强制中断（无 JSON）。
- 失败时 `error = { code, message, hint }`：`hint` 是给用户的下一步（中文），可直接转述。
- `restore_failed` 最严重：节点**没切回去**，请立刻提醒用户到 Clash Verge 里手动选回（`message` 里有原节点名）。
- 数字已取整（2 位小数，`totalScore` 1 位）。

## 5. 进度（stderr NDJSON）

`check` / `env` 运行时 stderr 每行一个 JSON：

```jsonc
{"type":"stage","stage":"test","index":1,"total":2,"nodeName":"香港 01","command":"check","ts":"…","text":"检测 1/2"}
{"type":"node_done","nodeName":"香港 01","index":1,"total":2,"stars":4.5,"totalScore":86.3,…}
```

- 只解析以 `{` 开头的行；其它是人类提示。
- `type`：`start` / `stage`（`connect|gate|precheck|switch|test|restore|probe`）/ `card` / `node_done` / `done`。
- 不需要时加 `--no-progress`。

## 6. 超时建议

| 命令 | 建议超时 |
|------|----------|
| help / install-shim / update-check | 15 s |
| discover / nodes | 30 s |
| gate | 45 s |
| env | 120 s |
| check current / check node | 180–240 s |
| check all / check nodes | **每个节点** 240 s ×（节点数）+ 60 s |

超时后先发 SIGINT（等价 Ctrl+C）：`check node/all` 会在当前节点测完后切回再退出（最多 90 s）；**不要直接 SIGKILL**——那样不会切回原节点。

## 7. 成本与副作用（务必告知用户）

- `check` 每个节点都会经代理真实下载做带宽抽样（数 MB～数十 MB），并访问 Netflix / ChatGPT / Gemini 等站点。
- `check node` / `check nodes` / `check all` 会**临时切换** Clash Verge 的选中节点，结束切回；期间用户的其它流量也走被测节点。
- `check all` 节点多时非常久（几十个节点 = 几十分钟）：**默认用 `--limit` / `--region` / `check nodes` 只测少量**，除非用户明确要全测。
- `--mock` 只替换节点列表，探针仍真实发请求。
- 同一时间只跑一个 CLI 实例（会互相切换节点）。

## 8. 常驻服务（可选）

需要多次调用时可用 `serve`（只绑 127.0.0.1，启动时 stdout 横幅 `data.token`）。路由与一次性命令一一对应：`POST /v1/discover|nodes|gate|env|check/current|check/all|check/node`；`/v1/check/all` body 可带 `{"limit":2,"region":"日本","names":[…]}`。请求串行；不推送进度。
