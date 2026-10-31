# Windows NSIS 模板：升级 tauri-cli 时如何重新同步

> 适用对象：`src-tauri/windows/installer.nsi`（M4b 起自带的 Tauri NSIS 模板）。
> 相关检查：`scripts/check-nsis-template.py`（CI 每次运行；`release-windows.yml` 演练时另跑 `--fetch-upstream`）。

## 现状

- `installer.nsi` = tauri-cli **v2.11.4** 的上游模板
  （`tauri-apps/tauri` @ `8909f221d1515955fc843808032bdc5d62209c96`，即 tag `tauri-cli-v2.11.4`，
  路径 `crates/tauri-bundler/src/bundle/windows/nsis/installer.nsi`）
  **加上唯一一段** `; >>> clash-assist M4b` … `; <<< clash-assist M4b`：把「删除应用数据」分支里的两行
  `RmDir /r "$APPDATA\${BUNDLEID}"` / `RmDir /r "$LOCALAPPDATA\${BUNDLEID}"` 换成
  `!insertmacro CA_DELETE_TAURI_APP_DATA`（`hooks.nsh`，不跟随 junction）。
- `check-nsis-template.py` 把这一段换回上游两行后，要求与上游逐字一致（固定 SHA-256），并要求
  `package.json` 的 `@tauri-apps/cli` 仍是 2.11.4。所以**单独升级 CLI 会让 CI 失败**，这是有意的。

## 升级步骤

1. 找到新版本 tag 指向的完整提交（不要手抄短 SHA）：
   ```sh
   gh api repos/tauri-apps/tauri/commits/tauri-cli-vX.Y.Z --jq .sha
   ```
2. 按该提交下载上游模板，覆盖 `src-tauri/windows/installer.nsi`：
   ```sh
   curl -fsSL "https://raw.githubusercontent.com/tauri-apps/tauri/<完整 SHA>/crates/tauri-bundler/src/bundle/windows/nsis/installer.nsi" \
     -o src-tauri/windows/installer.nsi
   sha256sum src-tauri/windows/installer.nsi   # 记下，用于第 4 步
   ```
3. 重新放回 M4b 段：在「勾选删除、非 /UPDATE」分支（`${If} $DeleteAppDataCheckboxState = 1` /
   `${AndIf} $UpdateMode <> 1`）内、`SetShellVarContext current` 之后，把那两行 `RmDir /r` 换成原来的
   M4b 段（照抄旧文件里的整段）。如果上游改了这两行或这个分支，先停下来评估，并同步改
   `check-nsis-template.py` 里的 `UPSTREAM_LINES` / `CONTEXT`。
4. 更新 `scripts/check-nsis-template.py`：`UPSTREAM_VERSION`、`UPSTREAM_COMMIT`（完整 SHA）、
   `UPSTREAM_SHA256`（第 2 步的值，LF 换行）。同时升级 `package.json` 的 `@tauri-apps/cli`，
   并检查 `release-windows.yml` 里 `test-nsis-hooks.ps1 -ExpectVersion`（CLI 下载的 makensis 版本）是否要改。
5. 本地验证：
   ```sh
   python3 scripts/check-nsis-template.py
   python3 scripts/check-nsis-template.py --self-test
   python3 scripts/check-nsis-template.py --fetch-upstream   # 联网，按 UPSTREAM_COMMIT 逐字比对
   ```
6. 在分支上触发 `release-windows.yml` 演练（`workflow_dispatch`，`dry_run=true`）：会重跑上游比对、
   `scan-nsis-rmdir.ps1`、冒烟测试和卸载 hook 实跑。`src-tauri/windows/` 与该 workflow 受 CODEOWNERS 保护，需所有者评审。
