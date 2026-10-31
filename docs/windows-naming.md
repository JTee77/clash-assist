# Windows 命名契约（V8）

> 来源：Windows 迁移方案 r4 §2.0 V8（已定稿，M0 交付物）。
> 适用范围：首个 Windows 版本起（x64，`x86_64-pc-windows-msvc`）。
> 下列名称被发布流水线（M4）、更新器（M5）、安装器钩子与冒烟测试共同依赖，
> **改动任何一项都必须同时改这几处，并在 PR 中说明。**
> NSIS 模板随 tauri-cli 升级的同步步骤见 [windows-nsis-template.md](windows-nsis-template.md)。

## 发布资产

| 项 | 名称 | 说明 |
|---|---|---|
| 安装包 | `Clash.Assist_<ver>_x64-setup.exe` | NSIS 按用户安装（`currentUser`，不需要管理员权限）。更新器的 `WinSetupX64` 只匹配这个名字 |
| 便携包 | `Clash.Assist_<ver>_x64-portable.zip` | 不提供一键更新，界面显示「前往发布页下载」 |
| 校验文件 | `SHA256SUMS.txt` | 与上面两个资产放在同一个 draft release 中 |

- `<ver>` 是不带 `v` 前缀的版本号，例如 tag `v0.3.0` 对应 `Clash.Assist_0.3.0_x64-setup.exe`。
- tag、`src-tauri/Cargo.toml`、`src-tauri/tauri.conf.json`、`package.json` 四处的版本必须一致。
- 文件名里的 `Clash.Assist` 来自 `productName`（`Clash Assist`）：GitHub 上传资产时会把空格换成 `.`。
- 对照：macOS 资产是 `Clash.Assist_<ver>_aarch64.dmg`（`MacDmgAarch64`），不受本契约影响。

## 安装位置与注册表

| 项 | 位置 | 依据 |
|---|---|---|
| 安装目录 | `%LOCALAPPDATA%\Clash Assist\` | Tauri NSIS 模板 `$LOCALAPPDATA\${PRODUCTNAME}`；`productName` 为 `Clash Assist` |
| Uninstall 键 | `HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\Clash Assist` | Tauri `installer.nsi` 的 `UNINSTKEY` 定义 |

- 更新器的安装类型检测（`install_kind`）从该 Uninstall 键读取 `InstallLocation`（带引号，需去掉）和 `MainBinaryName`，
  与 `current_exe()` 规范化后比较，并要求同目录下存在 `uninstall.exe`；其余情况一律判为便携版。
- 主程序文件名以注册表中的 `MainBinaryName` 为准，代码中不硬编码；M4 冒烟测试从注册表实测。

## 应用数据

| 项 | 位置 |
|---|---|
| 应用数据根目录 | `%LOCALAPPDATA%\ClashAssist\` |
| 日志 | `%LOCALAPPDATA%\ClashAssist\logs\` |
| 更新下载 | `%LOCALAPPDATA%\ClashAssist\updates\`（每次更新新建 `updates\<32 位随机十六进制>\`，私有 DACL） |
| 更新待完成标记 | `%LOCALAPPDATA%\ClashAssist\update-pending.json` |

- 注意应用数据目录名**不含空格**（`ClashAssist`），与安装目录（`Clash Assist`）不同，两者不能混用。
- 卸载时勾选「删除应用数据」才删除 `%LOCALAPPDATA%\ClashAssist\`；`/UPDATE` 模式下不删（`hooks.nsh` 带 `$UpdateMode` 守卫）。
- 不碰 Clash Verge Rev 的任何目录。

## 依赖这些名称的位置（落地时同步）

- 2.4 更新器：`parse_release_asset`（`WinSetupX64`）、`validate_asset_url`、`install_kind`、`prepare_update_dir`、`pending.rs`
- 3 `release-windows.yml`：改名、打便携 zip、生成 `SHA256SUMS.txt`
- 3 `src-tauri/windows/hooks.nsh`：卸载时删除应用数据
- `platform::app_log_dir()`：Windows 上改为 `%LOCALAPPDATA%\ClashAssist\logs`
- M4 冒烟测试 `scripts/smoke-windows.ps1`、M5 `updater-e2e.yml`

## 控制器命名管道（M1）

- 候选顺序：Verge yaml 的 `external-controller-pipe` → `\\.\pipe\verge-mihomo-production-{key}` → `\\.\pipe\verge-mihomo-sidecar-release-{key}`，其中 key = hex(sha256(当前用户 SID))。不带 key 的旧名字只在 yaml 写明时尝试（N-2）。前缀必须是小写的 `\\.\pipe\`。
- 信任判定见 `src-tauri/src/mihomo/trust.rs` 的 `decide_pipe`：
  - 属主白名单（按 PipeKind 区分）；
  - DACL 白名单（F3′）：只有本人、SYSTEM、BA 可以持有 `FILE_CREATE_PIPE_INSTANCE`、`WRITE_DAC`、`WRITE_OWNER`、`GENERIC_WRITE`、`GENERIC_ALL`、`MAXIMUM_ALLOWED`；
  - 完整性标签不低于 Medium，并且带 NO_WRITE_UP。
  - 以上三项在已连接的句柄上用一次 `GetSecurityInfo` 读出，判定通过之前不写任何字节。

### 接受的风险

- **R3：WRITE_DAC 注入后复原（TOCTOU）。**
  - 风险：安全描述符只在连接后读取一次。持有 `WRITE_DAC` 或 `WRITE_OWNER` 的主体可以先放宽 DACL、插入自己的管道实例，再把 DACL 改回原样。客户端读到的是复原之后的安全描述符，因此无法发现。
  - 为何接受：在 mihomo 默认 SDDL 和 Verge sidecar SDDL 下，`WRITE_DAC` / `WRITE_OWNER` 只授予受信主体（本人、SYSTEM、BA，或经 `GENERIC_ALL` 映射得到），而这些主体本来就能直接冒充控制器。任何把这些位授予其他主体的 DACL，都会在 F3′ 白名单检查中被拒绝。
  - 代码位置：`decide_pipe` 的函数注释和 F3′ 检查处都有对应注释。
