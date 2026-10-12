import { hintFor } from "./hints";

/** Stable stdout JSON shape for --cli / 本地脚本。 */
export type CliError = {
  code: string;
  message: string;
  /** 可执行的下一步（中文）；errEnvelope 按 code 自动填 */
  hint?: string;
};

/**
 * 一行 JSON。ok:false 时必有 error；部分失败（如 aborted / check_failed）
 * 还会带 data（已测到的结果、restoreError 等），便于脚本收尾。
 */
export type CliEnvelope<T = unknown> = {
  ok: boolean;
  version: string;
  command: string;
  ranAt: string;
  data?: T;
  error?: CliError;
};

export type CliCommandName =
  | "help"
  | "discover"
  | "nodes"
  | "gate"
  | "check"
  | "env"
  | "serve"
  | "install-shim";

export type CheckTarget =
  | { kind: "current" }
  | { kind: "node"; name: string }
  /** check all（可带筛选）与 check nodes <a> <b>…（names 非空）共用：输出 target:"all" */
  | { kind: "all"; names?: string[] };

/** nodes / check all / check nodes 的筛选。 */
export type NodeFilter = {
  /** 地区（逗号分隔多个）：等于/包含节点 region，或别名（HK、日本、🇯🇵…） */
  region: string | null;
  /** 节点名关键词（逗号分隔多个，任一命中；不区分大小写） */
  match: string | null;
  /** 最多取前 N 个（筛选之后） */
  limit: number | null;
};

export type ParsedCli = {
  /** 与 Rust `is_cli_mode` 同规则：含 --cli / --help / -h，或首个位置参数是已知子命令 */
  cliMode: boolean;
  /** 解析出的子命令；parseError 非空时不可信（调度直接报错） */
  command: CliCommandName;
  /** 用户写的第一个位置参数（未知命令时用于报错/回显） */
  commandToken: string | null;
  /** check 子命令目标 */
  checkTarget?: CheckTarget;
  /** --client 原值（未校验；校验在 dispatch 的 resolveCliClient） */
  clientId: string | null;
  /** 是否显式给了 --client（即使值非法） */
  clientGiven: boolean;
  mock: boolean;
  json: boolean;
  /** serve 的 --port；未给则为 null（serve 时用默认 17890） */
  port: number | null;
  /** 是否显式给了 --port（含非法值，非法时 parseError） */
  portGiven: boolean;
  filter: NodeFilter;
  /** install-shim 的 --dir */
  dir: string | null;
  /** NDJSON 进度事件写 stderr（默认开；--no-progress / --quiet 关闭） */
  progress: boolean;
  /** 未知命令 / 未知选项 / 缺值 / 多余参数；非空时 dispatch 返回 ok:false */
  parseError: CliError | null;
  /** 原始 argv（诊断用） */
  raw: string[];
};

export function okEnvelope<T>(
  command: string,
  data: T,
  version: string,
): CliEnvelope<T> {
  return {
    ok: true,
    version,
    command,
    ranAt: new Date().toISOString(),
    data,
  };
}

export function errEnvelope<T = never>(
  command: string,
  code: string,
  message: string,
  version: string,
  data?: T,
): CliEnvelope<T> {
  const env: CliEnvelope<T> = {
    ok: false,
    version,
    command,
    ranAt: new Date().toISOString(),
    error: withHint({ code, message }),
  };
  if (data !== undefined) env.data = data;
  return env;
}

/**
 * 进程退出码契约：envelope 正常输出时 0 = ok:true，1 = ok:false。
 * （强制中断——第二次 Ctrl+C 或切回等待超时——由 Rust 直接退出 130，且不输出 envelope。）
 */
export function cliExitCode(env: Pick<CliEnvelope, "ok">): 0 | 1 {
  return env.ok ? 0 : 1;
}

/** 按 code 补上 hint（已有 hint 不覆盖）。 */
export function withHint(e: CliError): CliError {
  if (e.hint) return e;
  const hint = hintFor(e.code);
  return hint ? { ...e, hint } : e;
}
