# 真实 GitHub release API 响应样本（windows-plan r4 §3 / §6 M5，S-R6）

这里只放**原样抓取**的 API 响应，不手写，也不改任何字段。

| 文件 | 内容 | 使用者 | 加入 | 抓取命令 |
|---|---|---|---|---|
| `release-clash-assist-v0.2.9.json` | 本仓库 v0.2.9（抓取时为 latest），1 个资产（aarch64 DMG），`immutable: false` | #61 e2e 回放（`scripts/updater-e2e-server.ps1 -Replay`）；#62 单测 | #61，2026-11-06 02:39（UTC+8） | `gh api -H "Accept: application/vnd.github+json" repos/JTee77/clash-assist/releases/tags/v0.2.9` |
| `releases-clash-assist.json` | 本仓库全部 26 个 release（v0.1.0–v0.2.9），数组 | #62 单测（`update_check/mod.rs` 中的 `FIXTURE_RELEASES`） | #62（dc40e94） | `gh api repos/JTee77/clash-assist/releases?per_page=100` |
| `release-mihomo-v1.19.33.json` | MetaCubeX/mihomo v1.19.33，127 个资产，`immutable: false` | #62 单测（`FIXTURE_MIHOMO`，`parse_release_asset` 大样本） | #62（dc40e94） | `gh api repos/MetaCubeX/mihomo/releases/tags/v1.19.33` |

后两条的抓取命令以 `src-tauri/src/update_check/mod.rs` 测试模块中的注释为准。

## 说明

- **回放规则**：e2e 用例在运行时由 `updater-e2e-server.ps1 -Replay` 改写 v0.2.9 样本，只改以下字段，其余原样保留，`-SelfTest` 会逐字段比对：
  - tag：`tag_name`，以及 `name`、`html_url`、`tarball_url`、`zipball_url` 中的旧 tag；
  - 唯一资产的 `name`、`browser_download_url`、`digest`、`size`；
  - `immutable`。
- **C-3**：0.2.x 发布于仓库开启 immutable releases 之前，`immutable` 都是 `false`，客户端一律拒绝一键更新。回放时 Happy 用例把它改成 `true`，ImmutableFalse 用例保持 `false`。
- **快照字段**：`download_count`、`updated_at` 等只是抓取时的快照，测试不应依赖其取值。
- **待补**：首个开启 immutability 后发布的 Windows release 出来后，应补抓一份 `immutable: true`、含 `x64-setup.exe` 资产的样本（真机清单第 17 项）。
