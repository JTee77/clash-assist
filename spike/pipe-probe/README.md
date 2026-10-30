# pipe-probe — V5 管道 spike 探针

对应 `windows-plan.md` r4 §4.1（V5 CI spike），以及 §2.1 规则 4（TCP 监听者校验）、§2.2（L0–L3）、Ellie r3 的 C-2 / S-R9。

- **只记录，不判定。** 探针从不输出“放行/拒绝/verified”之类结论；P1–P7 的判定由 workflow 脚本（Nate）或人工按本文“用例映射”读取 JSON 字段完成。
- 独立 crate：自带空 `[workspace]`，不是 `src-tauri` 的成员，不影响主应用构建与 `src-tauri/Cargo.lock`。依赖版本与主仓库 lock 一致：tokio `=1.53.1`、windows-sys `=0.61.2`（serde_json 1.0.151）。
- 本分支 `win/v5-pipe-spike` 永不合并到 main。workflow `.github/workflows/spike-v5-pipe.yml` 与标准用户脚本由 Nate 编写，本 README 是两者之间的 CLI 契约。
- 非 Windows 平台只编译出一个桩：打印错误并以退出码 3 结束（保证 Linux 上 `cargo check` 可用）。

## 构建

```powershell
# Windows runner（工具链由仓库根目录 rust-toolchain.toml 固定为 1.99.1）
cargo build --release --manifest-path spike/pipe-probe/Cargo.toml
# 产物：spike/pipe-probe/target/release/pipe-probe.exe
```

Linux 交叉编译（开发机自检用）：`cd spike/pipe-probe && cargo xwin build --release --target x86_64-pc-windows-msvc`。

单元测试（CLI 解析、HTTP 解析、JSON 形状，跨平台）：`cargo test --manifest-path spike/pipe-probe/Cargo.toml`。

## CLI 契约

```text
pipe-probe pipe <name> [--timeout-ms <ms>] [--out <file>]
pipe-probe tcp-probe <port> [--out <file>]
pipe-probe squat <name> <sddl|-> [--set-owner <sid|SY|BA>]... [--hold <secs>] [--ready-file <file>] [--out <file>]
pipe-probe --help | --version
```

| 参数 | 含义 |
|---|---|
| `<name>` | 管道名。可写裸名 `verge-mihomo-sidecar-release-<64hex>`，也可写完整的 `\\.\pipe\...`（前缀大小写不敏感）；裸名会自动补 `\\.\pipe\`。JSON 中记录的是补全后的完整名 |
| `--timeout-ms` | 仅 `pipe`。打开管道时 `ERROR_PIPE_BUSY` 的重试期限，以及 `GET /version` 往返的期限（各自独立计时）。默认 5000，范围 1..=120000 |
| `<port>` | 仅 `tcp-probe`。1..=65535 |
| `<sddl>` / `-` | 仅 `squat`。SDDL 字符串（经 `ConvertStringSecurityDescriptorToSecurityDescriptorW` 解析后作为 `SECURITY_ATTRIBUTES` 传给 `CreateNamedPipeW`）；`-` 表示不传安全描述符（使用进程令牌的默认属主与默认 DACL） |
| `--set-owner` | 仅 `squat`，可重复，按给出顺序执行。值可以是 `S-1-5-21-...`、`SY`、`BA` 或任何 SDDL SID 别名（经 `ConvertStringSidToSidW` 解析）。每个目标先 `SetSecurityInfo(OWNER_SECURITY_INFORMATION)`，再回读一次属主 |
| `--hold` | 仅 `squat`。创建（以及改属主）之后保持管道 N 秒（0..=3600，默认 0 = 立即退出），期间接受任意客户端连接，见下文“squat 的应答” |
| `--ready-file` | 仅 `squat`。管道创建完成、`--set-owner` 全部执行完后写入该文件（内容 `{"name":..,"pid":..}`），供脚本等待后再让另一个用户去探测 |
| `--out` | 把同一份 JSON 再写到文件（先写 `<file>.tmp` 再改名，读者看不到半个文件）。**以其他用户身份（`Start-Process -Credential` / 计划任务）运行时 stdout 往往拿不到，务必用 `--out`**，并确保该用户对目录有写权限 |

**退出码：** `0` = JSON 已输出（无论各步骤成功与否）；`1` = JSON 已输出到 stdout，但写 `--out` 失败；`2` = 命令行错误（无 JSON，stderr 有用法）；`3` = 非 Windows 平台。
**判定只看 JSON，不看退出码。**

`squat --hold` 期间，`--out` 会在“创建/改属主完成后”和“每个客户端连接处理完后”各写一次检查点（`"complete": false`），所以即使脚本中途 `Stop-Process`，文件中也有证据；正常结束时最后一次写入为 `"complete": true`。

## JSON 结构

stdout 上恰好一个 JSON 文档（pretty-printed）：

```jsonc
{
  "schema": "pipe-probe/v1",
  "probe_version": "0.1.0",
  "command": "pipe" | "tcp-probe" | "squat",
  "args": { ... },              // 解析后的参数（name 为补全后的完整管道名）
  "context": {                  // 运行探针的身份
    "pid": 1234,
    "session_id": 1,            // 或 {"error": {...}}
    "self": SidInfo,            // 本进程 TokenUser
    "elevated": false,          // TokenElevation
    "builtin_admins_enabled": false,  // CheckTokenMembership(BA)；P7 环境自检：ca_a/ca_b 应为 false
    "timestamp_unix_ms": 1761850000000
  },
  "complete": true,
  "steps": [ Step, ... ]        // 按执行顺序
}
```

**Step（每一步一个对象，缺省字段直接省略）：**

| 字段 | 类型 | 说明 |
|---|---|---|
| `step` | string | 步骤名，见下表 |
| `ok` | bool | 该 API 调用链是否成功（仅指调用成功，不代表“可信”） |
| `win32_error` | u32 | 失败时的 Win32 错误码（`GetLastError` 或 API 直接返回值） |
| `win32_error_name` | string | 常见错误码的符号名，如 `ERROR_ACCESS_DENIED`(5)、`ERROR_FILE_NOT_FOUND`(2)、`ERROR_PIPE_BUSY`(231)、`ERROR_INVALID_OWNER`(1307) |
| `ntstatus` | string | 仅 `l2_2`：`NtQuerySystemInformation` 的 NTSTATUS（如 `"0xC0000022"`）；此时 `win32_error` 为 `RtlNtStatusToDosError` 换算值 |
| `api` | string | 失败的具体 API（如 L3 中区分 `OpenProcess` / `OpenProcessToken` / `GetTokenInformation`） |
| `error` | string | 非 Win32 类失败（超时、PID 不在快照中、HTTP 解析失败、`skipped: ...`） |
| `pid` | u32 | 该步骤针对的进程 |
| `target` | string | `squat` 中对应的 `--set-owner` 目标原文 |
| `value` | any | 成功时（部分失败时也有）记录的数据 |

**SidInfo：** `{"sid": "S-1-5-...", "account": "DOMAIN\\name" | null, "is_self": bool|null, "is_system": bool|null, "is_builtin_admins": bool|null}`。`is_*` 只是与本进程 TokenUser、`S-1-5-18`、`S-1-5-32-544` 的 `EqualSid` 比较结果（事实，不是判定）；与 `ca_a`/`ca_b` 的比较请脚本自行用 `sid` 字段对照 `ci-standard-users.ps1` 取得的 SID。

### `pipe <name>` 的步骤（顺序固定）

| step | API | value（成功时） |
|---|---|---|
| `open` | tokio `ClientOptions::new().security_qos_flags(SECURITY_ANONYMOUS).open()`（即 `CreateFileW`，`SECURITY_SQOS_PRESENT\|SECURITY_ANONYMOUS`），遇 231 每 50ms 重试至 `--timeout-ms` | `{name, attempts, security_qos}`（失败时也带，同时有 `win32_error`） |
| `owner` | **L1**：同一句柄上 `GetSecurityInfo(SE_KERNEL_OBJECT, OWNER_SECURITY_INFORMATION)` | `{owner: SidInfo}` |
| `dacl` | 同一句柄 `GetSecurityInfo(OWNER\|GROUP\|DACL)` + SDDL 字符串 + `GetAclInformation`/`GetAce` 遍历（S-2 / S-D） | `{sddl, null_dacl, ace_count, aces: [{index, ace_type, ace_flags, kind: "allow"\|"deny", mask: "0x001F01FF", has_write_bits, trustee: SidInfo}]}`；非 allow/deny 类型的 ACE 只有 `index/ace_type/ace_flags`。`has_write_bits` = mask 含 `FILE_WRITE_DATA\|GENERIC_WRITE\|GENERIC_ALL` |
| `server_pid` | `GetNamedPipeServerProcessId` | `<pid>`（数字） |
| `l2_1` | **L2 ①** `OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION)` + `QueryFullProcessImageNameW` | `{path, file_name}` |
| `l2_2` | **L2 ②** `NtQuerySystemInformation(SystemProcessIdInformation=0x58)` | `{nt_path, file_name}`（`nt_path` 形如 `\Device\HarddiskVolume3\...\verge-mihomo.exe`） |
| `l2_3` | **L2 ③** `CreateToolhelp32Snapshot` + `Process32FirstW/NextW` | `{exe_file, parent_pid}`；找不到为 `error: "pid-not-in-snapshot"` |
| `l3` | **L3** `OpenProcess(PQLI)` + `OpenProcessToken(TOKEN_QUERY)` + `GetTokenInformation(TokenUser)` | `{token_user: SidInfo}` |
| `wts` | `WTSEnumerateProcessesExW(WTS_CURRENT_SERVER_HANDLE, level 1, WTS_ANY_SESSION)` 中该 PID 的行 | `{session_id, process_name, user_sid: SidInfo\|null, snapshot_size}`；找不到为 `error: "pid-not-in-wts-snapshot"` |
| `http_version` | 写入第一个字节：`GET /version HTTP/1.1`（`Host: localhost`、`Connection: close`），读到响应完整（Content-Length / chunked / 对端关闭）或超时，上限 64 KiB | `{status, reason, headers: [[k,v]...], body, body_json, bytes_read, elapsed_ms}` |

- `open` 失败时后续步骤**全部不出现**（P4 只看 `open`）。
- `server_pid` 失败时，`l2_1..wts` 以 `error: "skipped: no server pid"` 占位，`http_version` 仍然执行。
- **L2 ①②③ 总是全部执行并分别记录**（M1 生产代码是“① 返回 ACCESS_DENIED 才试 ②，再失败试 ③”的短路链；S-C 要求“记录 ①②③ 各自的结果”，所以探针不短路）。
- 所有 L1–L3/WTS 采集都在 `http_version` 之前完成（与 M-2“先校验后写入”的顺序一致）；探针不会因为任何结果而跳过 `http_version`。

### `tcp-probe <port>` 的步骤

**不建立任何连接、不发送任何请求**，只读表和进程信息（与 §2.1 规则 4 的数据采集相同）。

| step | value |
|---|---|
| `tcp_table_v4` | `GetExtendedTcpTable(AF_INET, TCP_TABLE_OWNER_PID_LISTENER)`：`{entries: [{local_addr, local_port, state, pid}], total_listeners}`，`entries` 只含该端口 |
| `tcp_table_v6` | 同上，`AF_INET6`，条目多一个 `local_scope_id` |
| `listener_pids` | 两张表中该端口所有 PID 去重升序 `[pid, ...]`；为空则到此结束 |
| 对每个 PID：`l2_1`、`l2_2`、`l2_3`、`l3`、`wts` | 与 `pipe` 相同，带 `pid` 字段 |

### `squat <name> <sddl|->` 的步骤

| step | 说明 |
|---|---|
| `sddl_parse` | 仅当给了 SDDL。失败则结束（`win32_error` 来自 `ConvertStringSecurityDescriptorToSecurityDescriptorW`） |
| `create` | `CreateNamedPipeW`（经 tokio `ServerOptions`）：`PIPE_ACCESS_DUPLEX`、`FILE_FLAG_FIRST_PIPE_INSTANCE`、`WRITE_OWNER`、`PIPE_REJECT_REMOTE_CLIENTS`、字节模式、`max_instances=1`。value `{name, security: "sddl"\|"default", first_pipe_instance, write_owner}`。**S-E ⑥ 的主要证据：`ok:false, win32_error:1307`**。同名管道已存在时通常为 5（first instance）或 231 |
| `owner_readback` | 创建成功后在服务端句柄上读回属主 `{owner: SidInfo}`（伪造若“成功”，这里能看到实际属主） |
| `set_owner` ×N | 每个 `--set-owner`：`SetSecurityInfo(OWNER)` 结果，`target` = 原文，value `{requested_owner: SidInfo}`。**S-E ⑤ 的主要证据：`win32_error:1307`**。目标无法解析时 `api: "ConvertStringSidToSidW"` |
| `owner_readback` ×N | 每次 `set_owner` 之后再读一次属主，带同一 `target` |
| `ready_file` | 给了 `--ready-file` 时写文件的结果 |
| `client_connected` ×M | `--hold` 期间每个客户端一条：`{client_pid, exchange: {request_bytes, request_line, read_error, responded, write_error}, disconnect_error}` |
| `hold` | `{seconds, elapsed_ms, connections: [...]}` |

`WRITE_OWNER` 是有意请求的：保证 `SetSecurityInfo` 失败只可能源于属主值本身（1307），而不是句柄缺少访问权（5）。

**squat 的应答：** `--hold` 期间对任何请求回复
`HTTP/1.1 503 Service Unavailable` + body `{"pipe_probe":"squat"}`，等客户端关闭后再 `DisconnectNamedPipe`，然后继续等下一个客户端。因此 `ca_a` 的 `pipe` 探测在 `http_version` 中看到 `status: 503` 且 `body_json.pipe_probe == "squat"`，即可确认连到的是抢注者。

## 用例映射（S-A..S-F → P1..P7）

记号：`$A` = `ca_a` SID，`$B` = `ca_b` SID，`$K` = `sha256(ca_a SID)` 的 64 位小写 hex；`step(x)` 指 `steps` 中 `step == "x"` 的那一项（tcp-probe 中按 `pid` 再筛选）。

| 用例 | 谁运行什么 | 读哪些字段 |
|---|---|---|
| **S-A** sidecar | mihomo 以 `ca_a` 运行（S-A SDDL）；`ca_a`：`pipe-probe pipe verge-mihomo-sidecar-release-$K --out sa.json` | **P1**：`step(http_version).ok && value.status == 200`。**P2**：`step(owner).ok && value.owner.sid == $A`。P3：`step(l2_1).ok` 且 `value.file_name` 不区分大小写等于 `verge-mihomo.exe` |
| **S-B** 其他用户 | 同一 mihomo；`ca_b`：`pipe-probe pipe verge-mihomo-sidecar-release-$K --out sb.json` | **P4**：`step(open).ok == false && win32_error == 5` |
| **S-C** 服务模式 | mihomo 以 SYSTEM 运行（管道 `verge-mihomo-production-$K`，外部控制器 `127.0.0.1:19098`）；`ca_a`：`pipe-probe pipe verge-mihomo-production-$K --out sc-pipe.json`；`ca_a`：`pipe-probe tcp-probe 19098 --out sc-tcp.json` | **P5**：`http_version.value.status == 200` 且 `owner.value.owner.is_system \|\| is_builtin_admins`。记录 `l2_1/l2_2/l2_3/l3/wts` 原样（预期 `l2_1.win32_error == 5`，②或③ ok）。tcp-probe：按 §2.1 规则 4 人工/脚本判定，预期为 `listener-unverified`（如 `l3.win32_error == 5`） |
| **S-D** 默认 SDDL | mihomo 以 `ca_a` 运行、不设 `LISTEN_NAMEDPIPE_SDDL`；`ca_b`：`pipe-probe pipe verge-mihomo-sidecar-release-$K --out sd.json` | 仅记录：`open` 结果；若连上，`dacl.value.aces`（预期 BU 的 ACE `has_write_bits == true`）与 `dacl.value.sddl` |
| **S-E ①②③** | `ca_b` 运行 mihomo，`LISTEN_NAMEDPIPE_SDDL` 依次为 `O:${A}D:(A;;GA;;;WD)`、`O:SYD:(A;;GA;;;WD)`、`O:BAD:(A;;GA;;;WD)`（`O:` 与 `D:` 之间无空格）；`ca_a`：`pipe-probe pipe verge-mihomo-sidecar-release-$K --out se-1.json` … | **P6**（旁证）：每次要么 `open` 失败（mihomo 已退出，多为 2），要么 `owner.value.owner.sid` **不在** {`$A`, `S-1-5-18`, `S-1-5-32-544`} 中 |
| **S-E ④ / P7** | `ca_b` 运行 mihomo，SDDL `D:(A;;GA;;;WD)`（无 `O:`）；`ca_a`：`pipe-probe pipe ... --out se-4.json` | **P7**：`open.ok && owner.value.owner.sid == $B` |
| **S-E ⑤** | `ca_b`：`pipe-probe squat verge-mihomo-sidecar-release-$K - --set-owner $A --set-owner SY --set-owner BA --out se-5.json` | **P6**（主证据）：三个 `set_owner` 均 `ok == false`，预期 `win32_error == 1307`；每个随后的 `owner_readback.value.owner.sid == $B` |
| **S-E ⑥** | `ca_b` 分三次运行（每次进程退出后管道即消失，可复用同一名称）：`pipe-probe squat verge-mihomo-sidecar-release-$K "O:${A}D:(A;;GA;;;WD)" --out se-6a.json`、`... "O:SYD:(A;;GA;;;WD)" --out se-6b.json`、`... "O:BAD:(A;;GA;;;WD)" --out se-6c.json` | **P6**（主证据，S-R9）：`create.ok == false && create.win32_error == 1307`。若某次 `create.ok == true`，看 `owner_readback.value.owner.sid`：等于目标即伪造成功 → **P6 不满足（管道路线 no-go）** |
| P7 备选（无需 mihomo） | `ca_b`：`pipe-probe squat <name> "D:(A;;GA;;;WD)" --hold 60 --ready-file r.json --out sq.json`；等 `r.json` 出现后 `ca_a`：`pipe-probe pipe <name> --out p7.json` | `p7.json`：`owner.value.owner.sid == $B`，`http_version.value.status == 503` 且 `body_json.pipe_probe == "squat"`；`sq.json` 中有对应 `client_connected` |
| **S-F** | `ca_b` 运行改名后的 `verge-mihomo.exe`（`external-controller: 127.0.0.1:19097` + secret）；`ca_a`：`pipe-probe tcp-probe 19097 --out sf.json` | 仅记录：`tcp_table_v4/v6.entries`、`listener_pids`，以及该 PID 的 `l2_1`、`l2_3`、`l3`、`wts.value.user_sid`（是否 `== $B` 决定能否启用 `foreign-user` 加速拒绝） |

环境自检（P7 前提、P4 前提）：所有 `ca_a`/`ca_b` 运行的 JSON 中 `context.self.sid` 应分别等于 `$A`/`$B`，`context.elevated == false`，`context.builtin_admins_enabled == false`。runneradmin 运行时这两项为 true，结果不代表普通用户。

## 已知限制 / 注意

- `wts` 用 `WTS_ANY_SESSION` 枚举所有会话；普通用户能看到多少其他会话的进程、`pUserSid` 是否为空，本身就是 S-F/S-C 要记录的事实。WTS 只作诊断（C-4），不作 verified 依据。
- `l2_2` 使用未公开的 `SystemProcessIdInformation`；结构体为自定义 `#[repr(C)] { ProcessId: HANDLE, ImageName: UNICODE_STRING }`，缓冲区按 UNICODE_STRING 上限一次给足。
- 探针请求 `SECURITY_ANONYMOUS`，因此抢注者的 `ImpersonateNamedPipeClient` 拿不到可用令牌；本探针不在 `squat` 端验证模拟级别（那是 M1 集成测试 ② 的内容）。
- 在 wine 下运行只用于检查 JSON 形状，安全语义无意义。
