/**
 * CLI serve：把 loopback HTTP 路由映射到与一次性 CLI 相同的 dispatchCli。
 * 会话级 --client / --mock 来自启动 argv；单次请求只带路径与（check/node 的）JSON body。
 */
import { dispatchCli, type CliDispatchOptions } from "./dispatch";
import { CLI_SERVE_DEFAULT_PORT } from "./parse";
import {
  errEnvelope,
  okEnvelope,
  type CliEnvelope,
  type NodeFilter,
  type ParsedCli,
} from "./types";

/** /v1/nodes 与 /v1/check/all 的 body：{ region?, match?, limit?, names? }（都可省；空 body = 不筛选） */
export function parseFilterBody(
  body: string,
  allowNames: boolean,
): { ok: true; filter: NodeFilter; names: string[] } | { ok: false; message: string } {
  const filter: NodeFilter = { region: null, match: null, limit: null };
  if (!body.trim()) return { ok: true, filter, names: [] };
  let raw: unknown;
  try {
    raw = JSON.parse(body);
  } catch {
    return { ok: false, message: "body 不是合法 JSON。" };
  }
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) {
    return { ok: false, message: "body 必须是 JSON 对象。" };
  }
  const o = raw as Record<string, unknown>;
  const allowed = ["region", "match", "limit", ...(allowNames ? ["names"] : [])];
  const extra = Object.keys(o).filter((k) => !allowed.includes(k));
  if (extra.length) {
    return { ok: false, message: `不认识的字段：${extra.join(", ")}（可用：${allowed.join(", ")}）` };
  }
  for (const k of ["region", "match"] as const) {
    const v = o[k];
    if (v === undefined || v === null) continue;
    const list = Array.isArray(v) ? v : [v];
    if (!list.every((x) => typeof x === "string")) {
      return { ok: false, message: `${k} 必须是字符串或字符串数组。` };
    }
    filter[k] = (list as string[]).map((x) => x.trim()).filter(Boolean).join(",") || null;
  }
  if (o.limit !== undefined && o.limit !== null) {
    if (typeof o.limit !== "number" || !Number.isInteger(o.limit) || o.limit < 1) {
      return { ok: false, message: "limit 必须是正整数。" };
    }
    filter.limit = o.limit;
  }
  let names: string[] = [];
  if (o.names !== undefined && o.names !== null) {
    if (!Array.isArray(o.names) || !o.names.every((x) => typeof x === "string")) {
      return { ok: false, message: "names 必须是字符串数组。" };
    }
    names = (o.names as string[]).map((x) => x.trim()).filter(Boolean);
  }
  return { ok: true, filter, names };
}

export { CLI_SERVE_DEFAULT_PORT };

export type ServeJob = {
  id: number;
  /** discover | gate | env | check/current | check/all | check/node */
  route: string;
  body: string;
};

export type ServeStartInfo = {
  host: string;
  port: number;
  token: string;
  baseUrl: string;
};

/** 从启动参数得到 serve 用的端口（未给 → 默认；已给则用解析值）。 */
export function resolveServePort(parsed: Pick<ParsedCli, "port" | "portGiven">): number {
  if (parsed.portGiven && parsed.port !== null) return parsed.port;
  return CLI_SERVE_DEFAULT_PORT;
}

/**
 * 把 HTTP 路由变成一次 ParsedCli（复用启动时的 client/mock）。
 * 未知路由 / check/node 缺 name → 返回 ok:false envelope（由调用方以 HTTP 200 回传，业务错误在 envelope 里）。
 */
export function parsedFromServeJob(
  base: ParsedCli,
  job: Pick<ServeJob, "route" | "body">,
): { parsed: ParsedCli; early?: CliEnvelope } {
  const shared: ParsedCli = {
    ...base,
    cliMode: true,
    parseError: null,
    json: true,
    // serve 会话不把 HTTP 请求再当成 serve 子命令
    command: "help",
    commandToken: null,
    checkTarget: undefined,
    filter: { region: null, match: null, limit: null },
    dir: null,
    // HTTP 回包只有一个 envelope；进度不经 HTTP 推送
    progress: false,
  };

  switch (job.route) {
    case "discover":
      return { parsed: { ...shared, command: "discover" } };
    case "nodes":
    case "check/all": {
      const isAll = job.route === "check/all";
      const r = parseFilterBody(job.body, isAll);
      if (!r.ok) {
        return {
          parsed: shared,
          early: errEnvelope(
            isAll ? "check" : "nodes",
            "invalid_body",
            `POST /v1/${job.route}：${r.message}`,
            version(),
          ),
        };
      }
      const withFilter = { ...shared, filter: r.filter };
      return isAll
        ? {
            parsed: {
              ...withFilter,
              command: "check",
              checkTarget: r.names.length ? { kind: "all", names: r.names } : { kind: "all" },
            },
          }
        : { parsed: { ...withFilter, command: "nodes" } };
    }
    case "gate":
      return { parsed: { ...shared, command: "gate" } };
    case "env":
      return { parsed: { ...shared, command: "env" } };
    case "check/current":
      return {
        parsed: {
          ...shared,
          command: "check",
          checkTarget: { kind: "current" },
        },
      };
    case "check/node": {
      let name = "";
      try {
        if (!job.body.trim()) {
          // 空 body → 缺 name
        } else {
          const raw = JSON.parse(job.body) as unknown;
          if (!raw || typeof raw !== "object" || Array.isArray(raw)) {
            return {
              parsed: shared,
              early: errEnvelope(
                "check",
                "invalid_body",
                'POST /v1/check/node 需要 JSON 对象：{"name":"节点名"}',
                version(),
              ),
            };
          }
          const n = (raw as { name?: unknown }).name;
          if (n === undefined) {
            // 合法 JSON 但没 name 字段 → node_name_required
          } else if (typeof n !== "string") {
            return {
              parsed: shared,
              early: errEnvelope(
                "check",
                "invalid_body",
                'POST /v1/check/node 的 name 必须是字符串',
                version(),
              ),
            };
          } else {
            name = n.trim();
          }
        }
      } catch {
        return {
          parsed: shared,
          early: errEnvelope(
            "check",
            "invalid_body",
            'POST /v1/check/node 的 body 不是合法 JSON。需要 {"name":"节点名"}',
            version(),
          ),
        };
      }
      if (!name) {
        return {
          parsed: shared,
          early: errEnvelope(
            "check",
            "node_name_required",
            'POST /v1/check/node 需要 JSON：{"name":"节点名"}',
            version(),
          ),
        };
      }
      return {
        parsed: {
          ...shared,
          command: "check",
          checkTarget: { kind: "node", name },
        },
      };
    }
    default:
      return {
        parsed: shared,
        early: errEnvelope(
          "serve",
          "unknown_route",
          `未知内部路由：${job.route}`,
          version(),
        ),
      };
  }
}

function version(): string {
  try {
    return typeof __APP_VERSION__ === "string" ? __APP_VERSION__ : "0.0.0";
  } catch {
    return "0.0.0";
  }
}

/** 处理一次 serve 请求（不含 /health，health 在 Rust）。 */
export async function dispatchServeJob(
  base: ParsedCli,
  job: ServeJob,
  opts: CliDispatchOptions = {},
): Promise<CliEnvelope> {
  const { parsed, early } = parsedFromServeJob(base, job);
  if (early) return early;
  return dispatchCli(parsed, opts);
}

/** stdout 就绪横幅（CliEnvelope）。token 会出现在 data 里——仅本机 loopback。 */
export function serveReadyEnvelope(
  info: ServeStartInfo,
  appVersion: string,
): CliEnvelope {
  return okEnvelope(
    "serve",
    {
      host: info.host,
      port: info.port,
      token: info.token,
      baseUrl: info.baseUrl,
      bind: "127.0.0.1",
      auth: ["Authorization: Bearer <token>", "X-Egress-Token: <token>"],
      routes: [
        "GET /health",
        "POST /v1/discover",
        "POST /v1/nodes",
        "POST /v1/gate",
        "POST /v1/env",
        "POST /v1/check/current",
        "POST /v1/check/all",
        "POST /v1/check/node",
      ],
      note: "请求串行处理（同一时间只跑一个 WebView 任务）。Ctrl+C 停止。",
    },
    appVersion,
  );
}
