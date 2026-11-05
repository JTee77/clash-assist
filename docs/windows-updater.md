# Windows 应用内更新（M5）

本文对应 windows-plan r4 §2.4 与 M5。实现见 `src-tauri/src/update_check/`：

| 文件 | 内容 |
|---|---|
| `mod.rs` | 共用部分：semver 比较、release 与资产校验、流式下载、check / 下载 / 重启三个入口的分发 |
| `windows.rs` | Windows 安装链：判定安装形态、私有更新目录、下载校验、启动安装程序 |
| `install.rs` | 判定逻辑（安装形态、ProductVersion、子进程观察），不调 FFI，全平台都跑单测 |
| `pending.rs` | 上次更新的标记文件与启动对账 |
| `e2e.rs` | `updater-e2e` feature 的开关（只有 e2e 构建会用到） |
| `macos.rs` | Mac 的 DMG 安装链（M5 只把它从原文件中拆出来，行为不变） |

## 流程

1. **检查**：读取 `releases/latest`。只有以下条件全部满足，才会给出 `one_click = true`：
   - `draft` 和 `prerelease` 都是 `false`；
   - `immutable` 为 JSON `true`（C-3：缺失、`null`、`"true"` 都会被拒绝）；
   - tag 是合法的 `v` 加 semver；
   - 资产名正好是 `Clash.Assist_{v}_x64-setup.exe`（V8）；
   - `state` 为 `uploaded`；
   - `size` 不超过 64 MiB；
   - `digest` 是 `sha256:` 加 64 位十六进制；
   - `browser_download_url` 与 `releases/download/{tag}/{name}` 逐字节相同；
   - 安装形态是 `nsis`。

   任何一项不满足，都只显示「去下载」（发布页）。
2. **安装形态（S-10）**：
   - 读取 `HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\Clash Assist` 下的 `InstallLocation`（可能带引号）和 `MainBinaryName`。
   - 拼出期望的 exe 路径，与 `current_exe()` 分别用 `GetFinalPathNameByHandleW` 规范化后比较。
   - 还要求同目录下有 `uninstall.exe`。
   - 不满足就是 `portable`：没有一键更新，按钮显示「前往发布页下载」。
3. **下载（M-4、S-R7、S-R10）**：
   - 安装包下载到 `%LOCALAPPDATA%\ClashAssist\updates\<32 位随机十六进制>\setup-<16 位随机十六进制>.exe`。
   - 每次新建这个目录，DACL 为 `D:P(A;OICI;FA;;;<本人 SID>)(A;OICI;FA;;;SY)`。
   - 父目录不得是重解析点，属主只能是本人、SYSTEM 或 Administrators。
   - 文件以 `create_new` 打开，共享模式为 0，经 `tokio::fs::File` 流式写入，写完后 `into_std` 并 drop。
   - 收到的字节数必须等于 API 给出的 `size`。
   - 然后以 `FILE_SHARE_READ` 重新打开只读句柄，遇到共享冲突时退避重试（200/500/1000 ms）。
   - 在阻塞线程上对这个句柄算 SHA-256，并检查 PE 的 `ProductVersion` 是否等于目标版本（S-6）。
   - 句柄一直保存在进程内，直到安装程序启动（S-R5），所以前端无法指定要运行哪个文件。
4. **立即重启（M-5、S-7）**：
   - 再比较一次 semver，然后写入 `%LOCALAPPDATA%\ClashAssist\update-pending.json`。
   - 用 `CreateProcess` 以 `DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP` 启动 `setup.exe /S /UPDATE /R`（不经 ShellExecute）。
   - 每 100 ms 观察一次，最长 2 s。安装程序以非零码退出，算失败：清理目录与标记，软件不退出，前端提示并打开发布页。否则本进程退出，等安装程序完成后重新打开。
5. **启动对账**：启动时读取标记文件。无论结果如何，标记文件与对应的工作目录都会被删除，所以提示只出现一次。
   - 当前版本等于 `to`：提示「已更新到 {v}」。
   - 当前版本仍等于 `from`：提示「上次更新未完成」，提供「重试」和「前往发布页」。
   - 标记超过 24 小时，或时间在未来 5 分钟以后：忽略。

日志写到 `%LOCALAPPDATA%\ClashAssist\logs\app.log`。下载日志会记录是否经过本地代理（S-14）。

## 前提：仓库开启 immutable releases

- C-3 要求 `immutable == true`，且这条要求不区分平台。Mac 的 DMG 链也一样（M5 决定）。
- 截至 M5，仓库的 0.2.x release 都是 `immutable: false`，真实快照见 `tests/fixtures/github-release/`。
- 因此，在仓库设置中开启 **immutable releases** 之前：
  - 新版客户端在 Mac 和 Windows 上都只会显示「去下载」；
  - 已发布的旧版 Mac 客户端不受影响，仍按旧规则工作。
- 开启后发布的第一个 release 需要确认两点：
  - `gh release view vX.Y.Z --json isImmutable` 为 `true`；
  - API 返回里 `immutable` 为 `true`。

## updater-e2e 契约（给 `updater-e2e.yml` 用）

e2e workflow 和本地服务器脚本不在 M5 范围内，由后续 PR 实现。这里列出客户端一侧已经固定的契约。

- **构建**：`pnpm tauri build -- --features updater-e2e`。release workflow 不得启用这个 feature，`check-release-workflow.py` 会检查。
- **API 重定向**：设置环境变量 `CLASH_ASSIST_E2E_API=http://127.0.0.1:<port>/release.json`。
  - 只接受这种形状：http、`127.0.0.1`、显式端口，没有用户信息、查询串和片段。
  - 其他值会被忽略，仍然请求 GitHub。
- **资产地址**：`browser_download_url` 必须正好是 `http://127.0.0.1:<port>/download/<tag>/<资产名>`，端口与 API 相同。其余校验（digest、size、immutable、资产名、ProductVersion）与生产构建完全一致。
- **代理**：请求本地回环地址时不走系统或 Clash 代理。
- **假版本**：`CLASH_ASSIST_FAKE_LATEST` 只在 debug 与 e2e 构建中存在。e2e 跑真实链路时不要设置它。
- **标记**：e2e 构建含有 `CLASH_ASSIST_UPDATER_E2E_BUILD`，扫描时用 `scan-exe-markers.ps1 -Build E2E`，找不到就失败。release 构建用 `-Build Release` 扫描，两种标记任何一个出现都失败。
- **可观察结果**：
  - `update-pending.json` 的写入与清理；
  - `app.log` 中 `[update]` 开头的行；
  - 重启后前端的「已更新到 {v}」提示，或者「上次更新未完成」提示。
  - 注意：Windows 版没有 CLI（`is_cli_mode` 恒为 false），只能从 GUI 和日志观察。
