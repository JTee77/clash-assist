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
  "gate",
  "check",
  "env",
  "serve",
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

/**
 * 解析进程 argv（可含二进制名）。严格模式：
 * - 未知子命令 → parseError `unknown_command`（不会静默变成 help）
 * - 未知选项 → `unknown_option`
 * - `--client` 缺值 → `missing_option_value`
 * - 子命令多余参数 → `unexpected_argument`
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
  let parseError: CliError | null = null;
  const fail = (e: CliError) => {
    if (!parseError) parseError = e; // 只报第一处错误（按出现顺序）
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
    if (t === "--cli") {
      cliMode = true;
    } else if (t === "--mock") {
      mock = true;
    } else if (t === "--json") {
      json = true;
    } else if (t === "--no-json") {
      json = false;
    } else if (t === "--help" || t === "-h") {
      cliMode = true;
      helpFlag = true;
    } else if (t === "--client" || t === "-c") {
      const v = tokens[i + 1];
      clientGiven = true;
      if (v === undefined || v === "" || v.startsWith("-")) {
        fail(
          err(
            "missing_option_value",
            `${t} 需要一个值，例如：${t} verge`,
          ),
        );
        clientId = null;
        i += 1;
        continue;
      }
      clientId = v;
      i += 2;
      continue;
    } else if (t.startsWith("--client=")) {
      clientGiven = true;
      const v = t.slice("--client=".length);
      if (!v) {
        fail(err("missing_option_value", "--client= 需要一个值，例如：--client=verge"));
        clientId = null;
      } else {
        clientId = v;
      }
    } else if (t === "--port" || t === "-p") {
      portGiven = true;
      const v = tokens[i + 1];
      if (v === undefined || v === "" || v.startsWith("-")) {
        fail(
          err(
            "missing_option_value",
            `${t} 需要一个端口号，例如：${t} ${CLI_SERVE_DEFAULT_PORT}（0 = 临时端口）`,
          ),
        );
        port = null;
        i += 1;
        continue;
      }
      const n = Number(v);
      if (!Number.isInteger(n) || n < 0 || n > 65535) {
        fail(
          err(
            "invalid_port",
            `--port 必须是 0–65535 的整数（0 = 临时端口），收到：${v}`,
          ),
        );
        port = null;
      } else {
        port = n;
      }
      i += 2;
      continue;
    } else if (t.startsWith("--port=")) {
      portGiven = true;
      const v = t.slice("--port=".length);
      if (!v) {
        fail(
          err(
            "missing_option_value",
            `--port= 需要一个端口号，例如：--port=${CLI_SERVE_DEFAULT_PORT}`,
          ),
        );
        port = null;
      } else {
        const n = Number(v);
        if (!Number.isInteger(n) || n < 0 || n > 65535) {
          fail(
            err(
              "invalid_port",
              `--port 必须是 0–65535 的整数（0 = 临时端口），收到：${v}`,
            ),
          );
          port = null;
        } else {
          port = n;
        }
      }
    } else {
      fail(
        err(
          "unknown_option",
          `未知选项：${t}。可用选项见 egress-checker --cli help`,
        ),
      );
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
      fail(
        err(
          "unknown_command",
          `未知命令：${head}。可用命令：${CLI_COMMANDS.join(" / ")}`,
        ),
      );
    }
    // 非 cliMode 且首词不是子命令：交给 GUI（Rust 侧同样不会进 CLI）。
  }

  const rest = positional.slice(1);
  const noExtra = (what: string) => {
    if (rest.length) {
      fail(
        err(
          "unexpected_argument",
          `${what} 不接受参数：${rest.join(" ")}`,
        ),
      );
    }
  };

  if (head !== undefined && isCommand(head)) {
    if (command === "check") {
      const sub = rest[0];
      const after = rest.slice(1);
      if (sub === undefined || sub === "current" || sub === "all") {
        checkTarget = { kind: sub === "all" ? "all" : "current" };
        if (after.length) {
          fail(
            err(
              "unexpected_argument",
              `check ${sub ?? "current"} 不接受参数：${after.join(" ")}`,
            ),
          );
        }
      } else if (sub === "node") {
        const name = after.join(" ").trim();
        checkTarget = { kind: "node", name };
        if (!name) {
          fail(
            err(
              "node_name_required",
              'check node 需要节点名称，例如：--cli check node "香港"',
            ),
          );
        }
      } else {
        // 简写：check <节点名>（不是 current/node/all 时）
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
    parseError,
    raw,
  };
}

export const CLI_HELP_TEXT = `Egress Checker CLI 0.1.17（与 GUI 共用 runner / egress / score）

用法：
  egress-checker --cli <command> [options]

命令：
  help                         显示本说明
  discover                     发现控制器并探测连接
  gate                         轻量门槛（连通 / 是否像未走代理）
  check current                测当前出口（完整深测）
  check node <名称>            测指定节点（完整深测，可临时切换后切回）
  check all                    测全部节点（完整深测，会逐个切换节点并切回）
  env                          环境泄漏检查
  serve                        本机 loopback HTTP（127.0.0.1），常驻直到 Ctrl+C

选项：
  --client, -c <id>            目前仅支持 verge（Clash Verge / Clash Verge Rev）
                               FlClash 即将支持，暂不可用
  --mock                       不连真实软件，用演示节点跑通管线（--client 可省略，默认 verge）
  --port, -p <N>               仅 serve：监听端口（默认 17890；0 = 临时端口，就绪时打印）
  --json                       stdout 输出一行 CliEnvelope JSON（默认）
  --no-json                    输出人类可读摘要（脚本请用 JSON；serve 就绪横幅仍同时打 stderr 人类文案 + stdout JSON）
  --help, -h                   同 help
  --                           之后的词都当作位置参数（节点名以 - 开头时用）

退出码：0 = ok:true；1 = ok:false（含未知命令/选项、参数错误、中断）。
中断：check all 收到 Ctrl+C / SIGTERM 时会在当前节点测完后停止并切回原节点；
      再按一次 Ctrl+C 立即退出（不切回，退出码 130）。
      serve：Ctrl+C 停止监听并退出（进行中的请求会尽量跑完/切回）。

示例：
  egress-checker --cli discover --client verge
  egress-checker --cli gate --client verge
  egress-checker --cli check current --client verge
  egress-checker --cli check node "香港" --client verge
  egress-checker --cli check all --mock --no-json
  egress-checker --cli env --client verge
  egress-checker --cli serve --mock --port 17890
  egress-checker --cli serve --client verge

说明：CLI 走隐藏窗 + WebView 调用与 GUI 相同的 Tauri 能力；需在 macOS 上运行本应用二进制（或 pnpm tauri dev -- --cli …）。
serve 只绑定 127.0.0.1，启动时打印一次性 token；请求需 Authorization: Bearer <token> 或 X-Egress-Token。
`;
