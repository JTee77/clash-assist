import type { CheckTarget, CliCommandName, CliError, ParsedCli } from "./types";
export type { ParsedCli } from "./types";

/**
 * CLI 子命令全集。
 * ⚠️ 必须与 Rust `src-tauri/src/lib.rs` 的 `CLI_VERBS` 完全一致
 * （`is_cli_mode` 靠它决定是否隐藏主窗走 CLI）。cli.test.ts 有同步断言。
 */
export const CLI_COMMANDS: readonly CliCommandName[] = [
  "help",
  "discover",
  "nodes",
  "gate",
  "check",
  "env",
  "serve",
  "install-shim",
  "update-check",
] as const;

/** serve 默认端口（与 Rust `cli_serve::DEFAULT_PORT` 一致）。 */
export const CLI_SERVE_DEFAULT_PORT = 17890;

const COMMANDS = new Set<string>(CLI_COMMANDS);

function isCommand(s: string): s is CliCommandName {
  return COMMANDS.has(s);
}

function dropBinaryName(argv: string[]): string[] {
  const tokens = [...argv];
  if (!tokens[0]) return tokens;
  const head = tokens[0]!;
  if (isCommand(head)) return tokens;
  // argv[0] 是二进制路径/名字（或垃圾）：只要不是 flag 就丢掉。
  if (!head.startsWith("-")) {
    tokens.shift();
  }
  return tokens;
}

function err(code: string, message: string): CliError {
  return { code, message };
}

/** 选项 → 允许的命令（未列出的选项对所有命令有效）。 */
const FILTER_COMMANDS = "nodes、check all、check nodes";

/**
 * 解析进程 argv（可含二进制名）。严格模式：
 * - 未知子命令 → parseError `unknown_command`（不会静默变成 help）
 * - 未知选项 → `unknown_option`
 * - 选项缺值 → `missing_option_value`
 * - 子命令多余参数 → `unexpected_argument`
 * - 选项配错命令 → `option_not_applicable` / `port_only_for_serve`
 * - `check node` 不带名字 → `node_name_required`
 * `--` 之后的所有词都按位置参数处理（节点名以 - 开头时用）。
 *
 * 是否进入 CLI（cliMode）与 Rust `is_cli_mode` 同一规则：
 * 含 `--cli` / `--help` / `-h`，或第一个位置参数是已知子命令。
 */
export function parseCliArgv(argv: string[]): ParsedCli {
  const tokens = dropBinaryName(argv);

  let cliMode = false;
  const raw = [...argv];
  const positional: string[] = [];
  let clientId: string | null = null;
  let clientGiven = false;
  let mock = false;
  let json = true; // CLI 默认 JSON；--no-json 关闭
  let helpFlag = false;
  let port: number | null = null;
  let portGiven = false;
  let region: string | null = null;
  let match: string | null = null;
  let limit: number | null = null;
  let dir: string | null = null;
  let progress = true;
  let cached = false;
  const given = new Set<string>();
  let parseError: CliError | null = null;
  const fail = (e: CliError) => {
    if (!parseError) parseError = e; // 只报第一处错误（按出现顺序）
  };

  /** 取值型选项：`--x v` / `-x v` / `--x=v`。返回 [值或 null, 消耗的 token 数]。 */
  const takeValue = (
    i: number,
    t: string,
    example: string,
  ): [string | null, number] => {
    const eq = t.indexOf("=");
    if (t.startsWith("--") && eq > 0) {
      const v = t.slice(eq + 1);
      if (!v) {
        fail(err("missing_option_value", `${t} 需要一个值，例如：${example}`));
        return [null, 1];
      }
      return [v, 1];
    }
    const v = tokens[i + 1];
    if (v === undefined || v === "" || v.startsWith("-")) {
      fail(err("missing_option_value", `${t} 需要一个值，例如：${example}`));
      return [null, 1];
    }
    return [v, 2];
  };

  const optName = (t: string) => {
    const eq = t.indexOf("=");
    return t.startsWith("--") && eq > 0 ? t.slice(0, eq) : t;
  };

  let i = 0;
  let endOfOptions = false;
  while (i < tokens.length) {
    const t = tokens[i]!;
    if (endOfOptions || !t.startsWith("-") || t === "-") {
      positional.push(t);
      i += 1;
      continue;
    }
    if (t === "--") {
      endOfOptions = true;
      i += 1;
      continue;
    }
    const name = optName(t);
    if (t === "--cli") {
      cliMode = true;
    } else if (t === "--mock") {
      mock = true;
    } else if (t === "--json") {
      json = true;
    } else if (t === "--no-json") {
      json = false;
    } else if (t === "--cached") {
      given.add("--cached");
      cached = true;
    } else if (t === "--no-progress" || t === "--quiet" || t === "-q") {
      progress = false;
    } else if (t === "--help" || t === "-h") {
      cliMode = true;
      helpFlag = true;
    } else if (name === "--client" || name === "-c") {
      clientGiven = true;
      const [v, n] = takeValue(i, t, `${name} verge`);
      clientId = v;
      i += n;
      continue;
    } else if (name === "--port" || name === "-p") {
      portGiven = true;
      const [v, n] = takeValue(i, t, `${name} ${CLI_SERVE_DEFAULT_PORT}（0 = 临时端口）`);
      port = null;
      if (v !== null) {
        const num = Number(v);
        if (!Number.isInteger(num) || num < 0 || num > 65535) {
          fail(err("invalid_port", `--port 必须是 0–65535 的整数（0 = 临时端口），收到：${v}`));
        } else {
          port = num;
        }
      }
      i += n;
      continue;
    } else if (name === "--region" || name === "--match") {
      given.add(name);
      const [v, n] = takeValue(i, t, name === "--region" ? "--region 日本" : "--match IPLC");
      if (name === "--region") region = v?.trim() || null;
      else match = v?.trim() || null;
      i += n;
      continue;
    } else if (name === "--limit" || name === "-n") {
      given.add("--limit");
      const [v, n] = takeValue(i, t, "--limit 3");
      if (v !== null) {
        const num = Number(v);
        if (!Number.isInteger(num) || num < 1) {
          fail(err("invalid_limit", `--limit 必须是正整数，收到：${v}`));
        } else {
          limit = num;
        }
      }
      i += n;
      continue;
    } else if (name === "--dir") {
      given.add("--dir");
      const [v, n] = takeValue(i, t, "--dir ~/.local/bin");
      dir = v;
      i += n;
      continue;
    } else {
      fail(err("unknown_option", `未知选项：${t}。可用选项见 clash-assist --cli help`));
    }
    i += 1;
  }

  let command: CliCommandName = "help";
  let commandToken: string | null = null;
  let checkTarget: CheckTarget | undefined;

  const head = positional[0];
  if (head !== undefined) {
    commandToken = head;
    if (isCommand(head)) {
      cliMode = true;
      command = head;
    } else if (cliMode) {
      fail(err("unknown_command", `未知命令：${head}。可用命令：${CLI_COMMANDS.join(" / ")}`));
    }
    // 非 cliMode 且首词不是子命令：交给 GUI（Rust 侧同样不会进 CLI）。
  }

  const rest = positional.slice(1);
  const noExtra = (what: string) => {
    if (rest.length) {
      fail(err("unexpected_argument", `${what} 不接受参数：${rest.join(" ")}`));
    }
  };

  if (head !== undefined && isCommand(head)) {
    if (command === "check") {
      const sub = rest[0];
      const after = rest.slice(1);
      if (sub === undefined || sub === "current" || sub === "all") {
        checkTarget = sub === "all" ? { kind: "all" } : { kind: "current" };
        if (after.length) {
          fail(err("unexpected_argument", `check ${sub ?? "current"} 不接受参数：${after.join(" ")}`));
        }
      } else if (sub === "nodes") {
        const names = after.map((s) => s.trim()).filter(Boolean);
        checkTarget = { kind: "all", names };
        if (!names.length) {
          fail(
            err(
              "node_name_required",
              'check nodes 需要至少一个节点名，例如：--cli check nodes "香港 01" "日本 02"',
            ),
          );
        }
      } else if (sub === "node") {
        const name = after.join(" ").trim();
        checkTarget = { kind: "node", name };
        if (!name) {
          fail(err("node_name_required", 'check node 需要节点名称，例如：--cli check node "香港"'));
        }
      } else {
        // 简写：check <节点名>（不是 current/node/nodes/all 时）
        checkTarget = { kind: "node", name: rest.join(" ").trim() };
      }
    } else {
      noExtra(command);
    }
  }

  if (helpFlag && !parseError) {
    command = "help";
    checkTarget = undefined;
  }

  if (!parseError && command !== "help") {
    const filterOk = command === "nodes" || (command === "check" && checkTarget?.kind === "all");
    for (const o of ["--region", "--match", "--limit"]) {
      if (given.has(o) && !filterOk) {
        fail(err("option_not_applicable", `${o} 只能用于 ${FILTER_COMMANDS}`));
      }
    }
    if (given.has("--dir") && command !== "install-shim") {
      fail(err("option_not_applicable", "--dir 只能用于 install-shim"));
    }
    if (given.has("--cached") && command !== "check" && command !== "env") {
      fail(err("option_not_applicable", "--cached 只能用于 check、env"));
    }
  }

  if (portGiven && command !== "serve" && !parseError) {
    fail(
      err(
        "port_only_for_serve",
        `--port 只能与 serve 一起用，例如：--cli serve --port ${CLI_SERVE_DEFAULT_PORT}`,
      ),
    );
  }

  return {
    cliMode,
    command,
    commandToken,
    checkTarget,
    clientId,
    clientGiven,
    mock,
    json,
    port,
    portGiven,
    filter: { region, match, limit },
    dir,
    progress,
    cached,
    parseError,
    raw,
  };
}

export const CLI_HELP_TEXT = `Clash Assist (Clash伴侣) CLI 0.2.3（与 GUI 共用 runner / egress / score）

用法：
  clash-assist --cli <command> [options]
  完整路径：/Applications/Clash Assist.app/Contents/MacOS/clash-assist
  短命令：先运行一次 install-shim，之后直接 clash-assist <command> …

命令：
  help                         显示本说明（JSON 时 data.spec 为机器可读清单）
  discover                     发现控制器、连接状态、当前节点（节点摘要最多 50 个）
  nodes                        全部节点（不截断；可 --region / --match / --limit）
  gate                         轻量门槛（连通 / 是否像未走代理）
  check current                测当前出口（完整深测）
  check node <名称>            测指定节点（完整深测，可临时切换后切回）
  check nodes <名1> <名2> …    测多个指定节点（逐个切换，结束切回）
  check all                    测全部节点（可 --region / --match / --limit 只测一部分）
  env                          环境泄漏检查
  serve                        本机 loopback HTTP（127.0.0.1），常驻直到 Ctrl+C
  install-shim                 把 clash-assist 短命令装到 PATH（/usr/local/bin 可写时，否则 ~/.local/bin）
  update-check                 查 GitHub 最新 release：current / latest / update_available / url

选项：
  --client, -c <id>            支持 verge（Clash Verge / Clash Verge Rev）或 flclash（FlClash）
  --mock                       不连真实软件，用演示节点跑通管线（--client 可省略，默认 verge）
  --region <地区[,地区]>       nodes / check all / check nodes：按地区筛选（日本、JP、HK、🇸🇬…）
  --match <词[,词]>            同上：节点名包含任一关键词（不区分大小写）
  --limit, -n <N>              同上：筛选后最多取前 N 个
  --cached                     仅 check / env：直接读取本地缓存的上次结果（与 GUI 互通，不发探针）
  --no-progress, --quiet, -q   不往 stderr 写进度（默认 check / env 每行一个 JSON 事件）
  --port, -p <N>               仅 serve：监听端口（默认 17890；0 = 临时端口，就绪时打印）
  --dir <路径>                 仅 install-shim：安装目录
  --json                       stdout 输出一行 CliEnvelope JSON（默认）
  --no-json                    输出人类可读摘要（脚本请用 JSON）
  --help, -h                   同 help
  --                           之后的词都当作位置参数（节点名以 - 开头时用）

输出：stdout 只有一行 JSON；失败时 error 带 code / message / hint（下一步怎么做）。
进度：stderr 每行一个 JSON（type=start|stage|card|node_done|done），可边跑边解析。
退出码：0 = ok:true；1 = ok:false；130 = 强制中断（无 JSON）。
中断：check all 收到 Ctrl+C / SIGTERM 时会在当前节点测完后停止并切回原节点；
      再按一次 Ctrl+C 立即退出（不切回，退出码 130）。
系统：check / env 需要 macOS 14+（更早系统直接报 os_unsupported）。

示例：
  clash-assist --cli discover --client verge
  clash-assist --cli nodes --client verge --region 日本
  clash-assist --cli gate --client verge
  clash-assist --cli check current --client verge
  clash-assist --cli check node "香港" --client verge
  clash-assist --cli check all --client verge --region HK --limit 3
  clash-assist --cli check nodes "香港 01" "日本 02" --client verge
  clash-assist --cli env --client verge
  clash-assist --cli serve --client verge
  clash-assist --cli install-shim
  clash-assist --cli update-check

说明：CLI 走隐藏窗 + WebView 调用与 GUI 相同的 Tauri 能力；需在 macOS 上运行本应用二进制（或 pnpm tauri dev -- --cli …）。
serve 只绑定 127.0.0.1，启动时打印一次性 token；请求需 Authorization: Bearer <token> 或 X-Egress-Token。
`;
