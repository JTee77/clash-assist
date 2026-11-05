# 真实 GitHub release API 响应样本（windows-plan r4 §3 / §6 M5，S-R6）

只放**原样抓取**的 API 响应，不手写、不改字段。用例需要的改写（tag、资产名、URL、digest、size、
`immutable`）在测试运行时由 `scripts/updater-e2e-server.ps1 -Replay` 完成，改写规则与逐字段比对见该脚本。

| 文件 | 来源 | 抓取时间 | 抓取命令 |
|---|---|---|---|
| `release-clash-assist-v0.2.9.json` | 本仓库已发布的 v0.2.9（当时的 latest） | 2026-11-06 02:39（UTC+8） | `gh api -H "Accept: application/vnd.github+json" repos/JTee77/clash-assist/releases/tags/v0.2.9` |

说明：

- v0.2.9 发布于仓库开启 immutable releases 之前，响应中 `immutable` 为 `false`，只有一个 macOS DMG 资产。
  按 C-3，这样的 release 会被 Windows 客户端拒绝一键更新；e2e 回放时把 `immutable` 改为 `true`（Happy）或保留 `false`（ImmutableFalse）。
- `download_count`、`updated_at` 等是抓取时的快照，测试不应依赖其取值。
- §6 M5 提到的 mihomo v1.19.33 样本用于 `parse_release_asset` 单元测试，由更新器核心（M5）添加。
- 首个开启 immutability 后发布的 Windows release 出来后，应补抓一份（`immutable: true`、含 `x64-setup.exe` 资产），见真机清单第 17 项。
