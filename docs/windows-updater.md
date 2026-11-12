# Windows 应用内更新（M5）

本文对应 windows-plan r4 §2.4 与 M5。实现见 `src-tauri/src/update_check/`：

| 文件 | 内容 |
|---|---|
| `mod.rs` | 共用部分：semver 比较、release 与资产校验、流式下载、check / 下载 / 重启三个入口的分发 |
| `windows.rs` | Windows 安装链：判定安装形态、私有更新目录、下载校验、启动安装程序 |
| `install.rs` | 判定逻辑（安装形态、ProductVersion、子进程观察），不调 FFI，全平台都跑单测 |
| `pending.rs` | 上次更新的标记文件与启动对账 |
| `platform_marker.rs` | 发布说明里的平台改动标记（某平台无变化不提示更新，文档 46） |
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

## 某平台无变化时不提示更新（文档 46）

两个平台共用版本号，每次发版仍要带上全部安装包（含 DMG）。发布说明标题下方有一条 HTML 注释（页面上不显示）：

```text
<!-- clash-assist:platforms macos=0.3.1 windows=0.3.2 -->
```

- 值是**该平台最近一次有改动的版本**。客户端当前版本不低于这个值，就不提示更新；更早的用户照常提示，直接更新到最新版。
- 值来自仓库里的 `release/platforms.json`：

  ```json
  { "macos": "0.3.1", "windows": "0.3.2" }
  ```

  **发布人在版本 PR 里改这个文件**（和改版本号同一个 PR）：本版有改动的平台填本版版本号，没改动的平台保留原值。不在草稿里手改发布说明。
- `release-windows.yml` 生成发布说明时把模板里的 `{{MACOS_VERSION}}` / `{{WINDOWS_VERSION}}` 换成文件里的值，再用 `check-release-notes.mjs --version <本版>` 校验。
- CI（`check-release-notes.mjs`，每次运行都会检查）要求：两项都是 `X.Y.Z`；不高于应用版本；至少一个等于应用版本（防止版本 PR 漏改）；不低于 0.3.1。应用版本不高于 0.3.1 时两项都必须等于应用版本——0.3.1 是首个带这项逻辑的版本，它本身两个平台都要正常发布。
- 客户端 fail open：标记缺失、格式错误、release 未通过上面的校验（如非 immutable）时一律照常提示。只对 0.3.1 及以后的客户端生效。
- 取舍：标记在发布说明里，而发布说明在发布后仍可编辑（immutable 只锁资产和 tag）。有 release 编辑权限的人可以借此让某个平台不提示更新；但这样的人本来就能做更糟的事（例如撤下发布），而且标记只能**压下提示**，改变不了安装包本身：下载地址和 SHA-256 摘要仍只来自不可变的资产。

## 前提：仓库开启 immutable releases

- C-3 要求 `immutable == true`，且这条要求不区分平台。Mac 的 DMG 链也一样（M5 决定）。
- 截至 M5，仓库的 0.2.x release 都是 `immutable: false`，真实快照见 `src-tauri/tests/fixtures/`（`release-clash-assist-v0.2.9.json`、`releases-clash-assist.json`）。
- 因此，在仓库设置中开启 **immutable releases** 之前：
  - 新版客户端在 Mac 和 Windows 上都只会显示「去下载」；
  - 已发布的旧版 Mac 客户端不受影响，仍按旧规则工作。
- 开启后发布的第一个 release 需要确认两点：
  - `gh release view vX.Y.Z --json isImmutable` 为 `true`；
  - API 返回里 `immutable` 为 `true`。

## 发布：只能走 publish-release workflow

- 发布 release（draft → 正式）**只能**通过 `.github/workflows/publish-release.yml`：
  1. 推 `vX.Y.Z` tag，等 `release-windows.yml` 生成 draft；
  2. 把 `Clash.Assist_X.Y.Z_aarch64.dmg` 手动上传到这个 draft；
  3. 运行 `gh workflow run publish-release.yml --ref vX.Y.Z -f tag=vX.Y.Z`，并在 `release` 环境里审批。
- 闸门 fail-closed：资产必须恰好是 setup.exe、便携 zip、`SHA256SUMS.txt`、aarch64 DMG 这四件，都已 uploaded 且带 GitHub digest，SHA256SUMS 与 Windows 资产的 digest 一致，发布说明里的平台标记与 tag 提交上的 `release/platforms.json` 一致。任何一项不符就不发布。规则见 `scripts/check-release-draft.py`。
- **网页上 release 编辑页的 Publish 按钮会绕过这个闸门**，仓库代码无法禁用它。不要在网页上点 Publish，也不要用 `gh release edit --draft=false`。
- 开启 immutable releases 后，发布了就不能再增删资产。绕过闸门发出的缺 DMG 或多出文件的 release 无法补救，只能另发新版本。

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
   - 尝试启动：`[update] launch_attempt pid=<当前进程 PID> to=<目标版本> path=<暂存安装程序路径>`。全部校验（size、digest、PE 版本、启动前 semver 复核）通过并写好标记之后、紧接 CreateProcess 之前写这一行。它只证明「尝试启动」，不证明 CreateProcess 成功，更不证明升级成功；不含 URL 或任何凭据。
     - **路径规范化**（`install::launch_log_path`）：只去掉 Win32 verbatim 前缀，`\\?\UNC\<x>` 变为 `\\<x>`，`\\?\<x>` 变为 `<x>`；其余字节原样保留，不改大小写、不换分隔符、不 trim。e2e 对 4688 的 `NewProcessName` 做同样处理后与它逐字节比较。
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
   - app.log 中恰好有一行 `launch_attempt`，其 `pid` 是旧进程、`to=0.3.1`，`path` 与 4688 映像按上面的规则规范化后逐字节相等；
   - 父进程是旧应用进程；
   - 命令行逐字为 `"<该路径>" /S /UPDATE /R`；
   - 启动时刻早于旧进程退出。
2. 安装后 DisplayVersion 与 exe 的 PE ProductVersion 都精确等于 `0.3.1`。
3. 有新的 `clash-assist.exe` 进程（`/R`），其 ProductVersion 也是 `0.3.1`。
4. `[update] pending outcome=updated from=0.3.0 to=0.3.1` 在整个用例中恰好出现一次，之后 `update-pending.json` 被删除。
5. 再启动一次（不设 e2e 环境变量）时，不再出现 pending 行，`updates\` 下也没有工作目录。
6. 安装包只下载一次，字节数等于 `size`；旧进程在启动安装程序之前没有 `[update] error code=` 行；`e2e auto result=launched`。

在 CreateProcess 成功之前失败的情况（例如 `installer_spawn_failed`、`no_staged`）满足不了第 1 条，因此不可能被判为通过。`installer launched` 行如果出现，只作记录。

### 安装程序非零退出（installer_exit_nonzero）与测试安装程序

`installer-exit-nonzero` 用例经应用的完整路径（check → 下载 → size / digest / PE 校验 → 持句柄 CreateProcess）启动一个**测试安装程序**，验证核心在 2 s 观察期内看到非零退出后的处理。没有生产旁路，也没有任意路径覆盖：测试安装程序和真实安装包一样，由回放的 release.json 以真实 size 与 SHA-256 提供。

- 来源：`scripts/updater-e2e-exit-nonzero.nsi`。`ProductVersion` 逐字为目标版本（`0.3.1`）。`.onInit` 第一条指令就设置退出码 42 并退出，不结束任何进程，不写注册表，也不改动任何文件。版本资源中含唯一标记 `CLASH_ASSIST_E2E_TEST_INSTALLER_EXIT_NONZERO`。
- 隔离：
  - 只在 `updater-e2e.yml` 的 e2e job 中用 runner 的 makensis 现场编译到 `runner.temp/e2e-test-installer`，不上传；
  - selftest job 的 `scripts/updater-e2e-isolation-check.py` 固定：只有白名单文件可以引用它，`release-windows.yml` 与 `tauri*.conf.json` 不引用它，upload-artifact 不上传它；
  - 运行时，`test-installer-isolation` 用例确认真实安装包、便携 zip 的全部文件中都没有这个标记。
- 断言：
  - 4688 中恰好创建一次，父进程是旧进程，映像与 `launch_attempt` 一致，命令行为 `"<路径>" /S /UPDATE /R`；
  - 4689 记录退出码 42，存活时间不到 2 s；
  - `[update] 安装程序：ExitedErr(Some(42))`，并且恰好一行 `[update] error code=installer_exit_nonzero`，`e2e auto result=error step=launch`；
  - 没有 `installer launched` 行；旧进程 5 s 后仍在运行；
  - 版本仍为 0.3.0，安装目录内容（文件、大小、时间、SHA-256）不变；
  - 标记与工作目录都已当场删除；安装包只下载一次。
  - `e2e auto result=error step=launch` 后的说明文字，与 `error code=installer_exit_nonzero` 行的说明文字逐字相同。也就是说，`relaunch_updated_app` 交给调用方的就是这个字符串。

### 前端回退契约（自动化分层测试）

安装程序非零退出后，应用要回落到发布页。这一项**不在**无界面 e2e 中验收：无界面触发不经前端，核心也不会为此写一条「浏览器已打开」的测试事件。它按层验收，每层只证明自己那一段：

| 层 | 测试 | 证明范围 |
|---|---|---|
| e2e（Windows runner） | `installer-exit-nonzero` | 经真实下载、校验、持句柄 CreateProcess，测试安装程序以 42 退出。核心报 `installer_exit_nonzero`，清理标记与工作目录，旧进程不退出，并把说明文字原样返回给调用方 |
| Rust（Windows CI） | `update_check::windows::tests::launch_blocking_nonzero_exit_maps_to_frontend_error_and_cleans_up` | 生产 `launch_blocking` 遇到非零退出（`xcopy.exe` 充当安装程序）时返回 `Err(String)`，以「安装程序退出码 」开头、以「未完成更新」结尾。经 tauri `InvokeError` 后是 JSON 字符串；标记与工作目录当场删除 |
| Rust（全平台） | `update_check::tests::relaunch_error_reaches_frontend_as_plain_string`、`relaunch_command_shape_tests::relaunch_command_returns_string_error` | `relaunch_updated_app` 与生产 Tauri 命令的类型都是 `Result<(), String>`；installer_exit_nonzero 的说明文字原样成为前端收到的 rejection 值（错误码只进日志） |
| 前端（vitest） | `src/hooks/useUpdateFlow.fallback.test.ts` | 驱动真实 `useUpdateFlow`（真实 reducer 与编排，只 mock Tauri invoke / listen，并用最小 hooks 运行时替代 React 渲染器），见下方列表 |
| 实机（M6） | 物理设备测试 | 默认浏览器真的打开发布页 |

`useUpdateFlow.fallback.test.ts` 覆盖的情形：
- `relaunch_updated_app` 以上述字符串 reject（或以 Error reject）时：
  - 恰好一次 `open_release_page({ url: <检查结果的 url> })`；
  - 停在 `fallback`，`error` 保留原文，主按钮为「去下载」；只有 `open_release_page` 成功才提示「已为你打开下载页」；
  - 不进入 `restarting` / `updated`，也不再次启动安装程序。
- `open_release_page` 自身 reject：`openReleasePage` resolve `false`（不产生未处理的 rejection），不算成功，installer 错误仍可见。提示「未能自动打开下载页」，展示检查结果里经发布页白名单校验的 URL，既可选中复制也可用「复制链接」按钮。剪贴板不可用时仍可手动复制文本。
- 请求尚未完成时提示「正在打开下载页…」，绝不提前显示成功；手动点击「去下载 / 前往发布页」也用同样的结果文案。重试成功后改为成功文案，不再展示 URL。
- `src/components/TitleBar.fallback.test.ts` 验证实际 `UpdatePill` JSX：三种语言的成功 / 失败 / 进行中文案、失败时显示的地址和复制行为；任意 URL 不会被展示。
- 文案（`update.fallbackOpened` / `update.fallbackOpenFailed` / `update.fallbackOpening` / `update.copyLink`）：

  | 语言 | 成功 | 失败 | 进行中 | 复制按钮 |
  |---|---|---|---|---|
  | 简体中文 | 已为你打开下载页 | 未能自动打开下载页 | 正在打开下载页… | 复制链接 |
  | 繁體中文 | 已為你開啟下載頁 | 未能自動開啟下載頁 | 正在開啟下載頁… | 複製連結 |
  | English | Opened download page | Couldn’t open the download page | Opening download page… | Copy link |

  命令成功只说明系统接受了打开请求，不替代 M6 的真实浏览器测试。地址只来自 `check_update` 的 `url`（Rust 已校验，前端展示时再次白名单校验），没有新增任意地址入口。
- relaunch 成功：不回退，不打开发布页。

e2e 的 cases 脚本把这一项记为 `frontend-fallback-contract`，状态为 `layered`：不计为通过，也不计为失败。

另外，降级由两层拦住：

- 客户端在 check 和启动安装程序之前都要求目标版本严格新于当前版本，并且 PE 版本必须等于 tag。
- 安装程序本身在静默模式下也拒绝降级（`installer.nsi` 的 M5 段，见 [windows-nsis-template.md](windows-nsis-template.md)）。所以「已装 0.3.1，用 /S 或 /S /UPDATE 运行 0.3.0 安装包」会返回非零码，不会降级。
