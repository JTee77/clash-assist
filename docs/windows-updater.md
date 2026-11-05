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
- 截至 M5，仓库的 0.2.x release 都是 `immutable: false`，真实快照见 `src-tauri/tests/fixtures/`（`release-clash-assist-v0.2.9.json`、`releases-clash-assist.json`）。
- 因此，在仓库设置中开启 **immutable releases** 之前：
  - 新版客户端在 Mac 和 Windows 上都只会显示「去下载」；
  - 已发布的旧版 Mac 客户端不受影响，仍按旧规则工作。
- 开启后发布的第一个 release 需要确认两点：
  - `gh release view vX.Y.Z --json isImmutable` 为 `true`；
  - API 返回里 `immutable` 为 `true`。

## e2e 接口约定

`updater-e2e.yml` 和本地服务器脚本属于 #61（Nate）。下面是客户端一侧固定的接口，只在 `--features updater-e2e` 构建中存在。release workflow 不得启用这个 feature，`check-release-workflow.py` 会检查。

1. **端点覆盖**：`CLASH_ASSIST_E2E_API` 取 release.json 的完整 URL，只接受 `http://127.0.0.1:<端口>/release.json` 这一种形状：
   - 必须是 http、`127.0.0.1`，并写明端口；
   - 路径只能是 `/release.json`；
   - 不能带用户信息、查询串或片段。

   其他值一律忽略，客户端照旧请求 GitHub。生产构建里这个环境变量名根本不存在。
2. **下载白名单**：`browser_download_url` 必须逐字节等于 `http://127.0.0.1:<同一端口>/JTee77/clash-assist/releases/download/<tag>/<资产名>`，路径形状与 GitHub 相同，也就是服务器脚本的默认 `DownloadPath`。
   - 旧的短路径 `/download/<tag>/<name>`、其他 owner 或 repo 都会被拒绝。
   - **不模拟 302**：重定向策略只允许跳到 GitHub 的 https 主机（`release-assets.githubusercontent.com` 等），由单测 `redirect_targets_limited_to_github_https` 覆盖。本地服务器返回任何 3xx 都会导致下载失败（`download_failed`），这本身也可以作为一条反例。
   - 请求 127.0.0.1 时不走代理。
3. **无界面触发**：设置 `CLASH_ASSIST_E2E_AUTO_UPDATE=1`。只有同时设置了合法的 `CLASH_ASSIST_E2E_API` 才生效，并且仅限 Windows。
   - 启动后依次执行 check → `download_and_verify` → `launch_installer`，与用户先点「一键更新」再点「立即重启」走同一段代码。
   - 成功时启动安装程序（`/S /UPDATE /R`），本进程退出，安装程序完成后重新打开新版本。
   - 失败时进程**不退出**，版本不变。
4. **可断言的信号**：都写在 `%LOCALAPPDATA%\ClashAssist\logs\app.log`，行格式固定，说明文字以后可能调整。
   - 每次拒绝或失败写一行：`[update] error code=<码> <说明>`。
   - check：`[update] check current=… latest=… update_available=… install_kind=… one_click=… reason=<码|->`
   - 触发结果：`[update] e2e auto result=launched|no_one_click reason=<码>|error step=check|download|launch …`
   - 成功启动：`[update] installer launched to=<版本> fate=StillRunning|ExitedOk，本进程退出`。这一行在 2 s 观察期结束后才写，安装程序可能在此之前结束本进程，所以**不保证出现**，也不能作为升级成功的证据（见下文「成功路径的证据」）。

   | 情形 | 码 | 出现在 |
   |---|---|---|
   | `immutable` 不是 JSON `true` | `immutable_false` | check（`one_click=false reason=immutable_false`） |
   | draft / prerelease / tag 不合法 | `release_draft` / `release_prerelease` / `release_tag_invalid` | check |
   | 资产缺失、state、size 字段、digest 字段、URL 不合规 | `asset_missing` / `asset_state` / `asset_size_invalid` / `asset_too_large` / `asset_digest_invalid` / `asset_url_rejected` | check |
   | 已是最新，或 tag 不比当前新（semver） | `up_to_date`（check）/ `not_newer`（启动前复核） | check / launch |
   | 便携版 | `portable` | check / download |
   | 实际字节数 ≠ API `size` | `size_mismatch` | download |
   | SHA-256 ≠ `digest` | `digest_mismatch` | download |
   | PE ProductVersion ≠ tag 版本 | `pe_version_mismatch` | download |
   | 网络或 HTTP 错误、重定向被拒 | `download_failed` | download |
   | 安装程序在 2 s 内以非零码退出（例如被 NSIS 拒绝的降级） | `installer_exit_nonzero` | launch |
   | 启动安装程序失败 / 没有已校验的安装包 | `installer_spawn_failed` / `no_staged` | launch |

   单测 `rejection_codes_are_stable` 固定这些码。
5. **从进程外读 install_kind / one_click**：Windows 版没有 CLI，所以从 app.log 读取 `[update] check …` 那一行（见第 4 点）。便携版会是 `install_kind=portable one_click=false reason=portable`（release 合规时），或者是 release 本身被拒的那个码。
6. **标记与目录**：
   - 标记文件：`%LOCALAPPDATA%\ClashAssist\update-pending.json`。点「立即重启」时，在启动安装程序之前写入。下次启动对账后无论结果如何都删除，并写一行 `[update] pending outcome=updated|unfinished|none from=… to=…`。如果安装程序启动失败或在观察期内以非零码退出，当场删除。
   - 工作目录：`%LOCALAPPDATA%\ClashAssist\updates\<32 位十六进制>\`。
     - 下载或校验失败时当场删除；
     - 启动失败时当场删除；
     - 对账时删除标记所指的那个目录；
     - 每次启动还会删除 `updates\` 下所有名字符合规则的目录（下载后没重启等情况留下的）。仍被占用的（安装程序还没退出）留到下一次启动再删。
7. **PE 版本规则**：比较的是安装包 `StringFileInfo` 里的 `ProductVersion` 字符串。去掉末尾的 NUL 和首尾空白后，必须与 tag 去掉 `v` 的版本逐字相等，例如 `0.3.1`。`0.3.1.0` 会被拒绝。Tauri 模板写入的是 `VIAddVersionKey "ProductVersion" "${VERSION}"`，即三段号；四段的 `VIProductVersion` 不参与比较。
8. **夹具**：
   - `src-tauri/tests/fixtures/release-clash-assist-v0.2.9.json` 与 #61 是同一个文件（逐字节相同）。
   - M5 另外加了 `release-mihomo-v1.19.33.json`（`parse_release_asset` 的大样本，127 个资产）和 `releases-clash-assist.json`（26 个历史 release）。
   - 真实的 v0.2.9 是 `immutable: false`，客户端一律拒绝一键更新（C-3）；Happy 用例需要回放时改成 `true`。

### 成功路径的证据

一键更新成功时，旧进程不一定来得及写任何「已启动」日志：
- 安装程序 `Section Install` 的 `CheckIfAppIsRunning`（`installer.nsi`）会结束正在运行的旧进程，可能发生在 2 s 观察期之内；
- 在观察期之前写日志只能证明 CreateProcess 成功，不能证明升级成功；
- 在观察期之后写的 `installer launched` 行不保证出现。

客户端**不会**为了留下日志而推迟、绕过安装程序结束旧进程，也不改变启动语义。e2e 的 happy 用例以下列条件**同时成立**判定成功，缺一不可：

1. 进程创建审计（Security 4688，含命令行）中恰好有 1 次安装程序启动，并且：
   - 映像为 `%LOCALAPPDATA%\ClashAssist\updates\<32 位十六进制>\setup-<16 位十六进制>.exe`；
   - 父进程是旧应用进程；
   - 命令行逐字为 `"<该路径>" /S /UPDATE /R`；
   - 启动时刻早于旧进程退出。
2. 安装后 DisplayVersion 与 exe 的 PE ProductVersion 都精确等于 `0.3.1`。
3. 有新的 `clash-assist.exe` 进程（`/R`），其 ProductVersion 也是 `0.3.1`。
4. `[update] pending outcome=updated from=0.3.0 to=0.3.1` 在整个用例中恰好出现一次，之后 `update-pending.json` 被删除。
5. 再启动一次（不设 e2e 环境变量）时，不再出现 pending 行，`updates\` 下也没有工作目录。
6. 安装包只下载一次，字节数等于 `size`；旧进程在启动安装程序之前没有 `[update] error code=` 行；`e2e auto result=launched`。

在 CreateProcess 成功之前失败的情况（例如 `installer_spawn_failed`、`no_staged`）满足不了第 1 条，因此不可能被判为通过。`installer launched` 行如果出现，只作记录。

另外，降级由两层拦住：

- 客户端在 check 和启动安装程序之前都要求目标版本严格新于当前版本，并且 PE 版本必须等于 tag。
- 安装程序本身在静默模式下也拒绝降级（`installer.nsi` 的 M5 段，见 [windows-nsis-template.md](windows-nsis-template.md)）。所以「已装 0.3.1，用 /S 或 /S /UPDATE 运行 0.3.0 安装包」会返回非零码，不会降级。
