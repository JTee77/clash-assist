/**
 * CLI 命令调度：与 GUI 共用 mihomo / runner / egress / score。
 * 无 React；结果收成 CliEnvelope。
 *
 * 原则：不静默兜底。参数错误、非法 --client、门槛不过、检测失败、切回失败、
 * 被中断 —— 一律 ok:false + 明确 error.code（见 docs/CLI.md「错误码」）。
 */
import { runNodeDiagnostics, type CheckCard, type EgressReport } from "../egress";
import {
  clientLabel,
  defaultConfig,
  detectRegion,
  discoverAndProbe,
  getProxies,
  isClientId,
  mockNodes,
  normalizeClientId,
  type ClientId,
  type ConnectionState,
  type ProxyNode,
} from "../mihomo";
import { runEnv, testAll, testOne, type RunnerHooks } from "../runner";
import {
  runLightGate,
  scoreNodeFromCards,
  type GateResult,
  type NodeScoreResult,
  type NodeStars,
} from "../score";
import { CLI_HELP_TEXT, type ParsedCli } from "./parse";
import { CLI_SPEC } from "./spec";
import {
  errEnvelope,
  okEnvelope,
  type CliEnvelope,
  type CliError,
  type NodeFilter,
} from "./types";

/** discover 的 data.nodes 最多列这么多个（nodeCount 仍是总数）。 */
export const CLI_NODES_LIMIT = 50;

/** 安装到 /Applications 后的二进制完整路径（文档 / install-shim 输出用）。 */
export const CLI_APP_BINARY = "/Applications/Egress Checker.app/Contents/MacOS/egress-checker";

/** 唯一受支持软件的提示（与 GUI 选择器一致：只有 Clash Verge 可选）。 */
export const CLI_ONLY_VERGE = "当前仅支持 Clash Verge（--client verge）。";

function version(): string {
  try {
    return typeof __APP_VERSION__ === "string" ? __APP_VERSION__ : "0.0.0";
  } catch {
    return "0.0.0";
  }
}

// ───────────────────────── data shapes（docs/CLI.md 同步） ─────────────────────────

export type CliScore = {
  stars: NodeStars;
  totalScore: number;
  blurb: string;
};

export type DiscoverData = {
  clientId: ClientId;
  status: ConnectionState["status"];
  message: string;
  currentProxy: string | null;
  /** currentProxy 为 null 时的原因（中文）；识别到时为 null */
  currentProxyReason: string | null;
  usingMock: boolean;
  config: {
    host: string;
    port: number;
    mixedPort: number;
    source: string;
    sockPath: string | null;
  } | null;
  nodeCount: number;
  /** nodeCount > CLI_NODES_LIMIT 时为 true，nodes 只含前 50 个 */
  nodesTruncated: boolean;
  /** 已连上但读节点列表失败时的原因；否则 null */
  nodesError: string | null;
  nodes: { name: string; type: string; region: string }[];
};

export type NodesData = {
  clientId: ClientId;
  status: ConnectionState["status"];
  currentProxy: string | null;
  /** 全部节点数（筛选前） */
  total: number;
  /** 本次返回的节点数（筛选 + limit 后） */
  count: number;
  filter: NodeFilter;
  nodesError: string | null;
  nodes: CliNodeRow[];
};

export type CliNodeRow = {
  name: string;
  type: string;
  region: string;
  /** 是否为当前在用节点 */
  current: boolean;
  /** 客户端最近一次测速（ms；0 = 失败；null = 没测过） */
  lastDelay: number | null;
};

export type GateData = {
  clientId: ClientId;
  connectionStatus: ConnectionState["status"];
  gate: GateResult;
};

export type EnvData = {
  clientId: ClientId;
  cards: CheckCard[];
};

export type CheckCurrentData = {
  target: "current";
  clientId: ClientId;
  nodeName: string;
  score: CliScore;
  cards: CheckCard[];
  note: string;
};

export type CheckNodeData = {
  target: "node";
  clientId: ClientId;
  nodeName: string;
  score: CliScore | null;
  cards: CheckCard[];
  /** 过程提示（如找不到策略组）；与 restoreError 分开 */
  hint: string | null;
  /** 临时切换后没能切回原节点时的报错；成功或未切换为 null */
  restoreError: string | null;
  aborted: boolean;
  gate: GateResult;
};

export type CheckAllData = {
  target: "all";
  clientId: ClientId;
  count: number;
  /** 本次纳入检测的节点（筛选 / check nodes 之后）；未筛选时为 null（= 全部） */
  selection: { filter: NodeFilter; names: string[] | null; selected: string[] } | null;
  scores: (CliScore & { nodeName: string })[];
  hint: string | null;
  restoreError: string | null;
  aborted: boolean;
  /** 已连上但读节点列表失败时的原因；否则 null */
  nodesError: string | null;
  gate: GateResult;
};

export type CheckData = CheckCurrentData | CheckNodeData | CheckAllData;

// ───────────────────────── client gate ─────────────────────────

export type ClientResolution =
  | { ok: true; clientId: ClientId }
  | { ok: false; error: CliError };

/**
 * --client 闸门，与 GUI 的 normalizeClientId 同一判定（目前只有 verge）。
 * - 未给 --client：--mock 时默认 verge；否则 client_required
 * - 给了但不受支持（flclash / clashx_meta / … / 任意未知值）：client_unsupported，
 *   --mock 也不例外（不静默回落到 verge）
 */
export function resolveCliClient(
  parsed: Pick<ParsedCli, "clientId" | "clientGiven" | "mock">,
): ClientResolution {
  if (!parsed.clientGiven) {
    if (parsed.mock) return { ok: true, clientId: "verge" };
    return {
      ok: false,
      error: {
        code: "client_required",
        message: `请用 --client verge 指定软件，或加 --mock 用演示数据。${CLI_ONLY_VERGE}`,
      },
    };
  }
  const raw = parsed.clientId;
  if (!raw) {
    return {
      ok: false,
      error: {
        code: "client_required",
        message: `--client 需要一个值。${CLI_ONLY_VERGE}`,
      },
    };
  }
  const id = normalizeClientId(raw);
  if (id) return { ok: true, clientId: id };
  let message: string;
  if (raw === "flclash") {
    message = `${CLI_ONLY_VERGE}FlClash 即将支持，暂不可用。`;
  } else if (isClientId(raw)) {
    message = `${CLI_ONLY_VERGE}「${raw}」暂不支持。`;
  } else {
    message = `未知软件「${raw}」。${CLI_ONLY_VERGE}`;
  }
  return { ok: false, error: { code: "client_unsupported", message } };
}

// ───────────────────────── node lookup ─────────────────────────

export type NodeResolution =
  | { ok: true; node: ProxyNode }
  | { ok: false; error: CliError };

/**
 * 按名找节点：先全名精确匹配，再唯一的包含匹配。
 * 找不到 / 多个候选 / 列表为空都报错，绝不拿一个不存在的名字去切换。
 */
export function resolveCliNode(
  nodes: ProxyNode[],
  name: string,
  nodesError: string | null = null,
): NodeResolution {
  if (!nodes.length) {
    return {
      ok: false,
      error: {
        code: "nodes_unavailable",
        message: `没有读到节点列表，无法按名称找节点。${nodesError ? `原因：${nodesError}` : "请确认软件里已加载订阅。"}`,
      },
    };
  }
  const exact = nodes.find((n) => n.name === name);
  if (exact) return { ok: true, node: exact };
  const partial = nodes.filter((n) => n.name.includes(name));
  if (partial.length === 1) return { ok: true, node: partial[0]! };
  if (partial.length > 1) {
    const shown = partial.slice(0, 10).map((n) => `「${n.name}」`).join("、");
    const more = partial.length > 10 ? ` 等 ${partial.length} 个` : "";
    return {
      ok: false,
      error: {
        code: "node_ambiguous",
        message: `「${name}」匹配到多个节点：${shown}${more}。请写全名。`,
      },
    };
  }
  return {
    ok: false,
    error: {
      code: "node_not_found",
      message: `找不到节点「${name}」（共 ${nodes.length} 个节点）。可先 discover 查看名称。`,
    },
  };
}

/** 该命令是否可能切换节点（需要「中断后先切回」而不能立即退出）。 */
export function cliCommandSwitchesNodes(parsed: ParsedCli): boolean {
  if (parsed.parseError || parsed.command !== "check") return false;
  const k = parsed.checkTarget?.kind ?? "current";
  return k === "all" || k === "node";
}

/** check node / all 被中断时的 error.message；切回结果看 data.restoreError。 */
export const CLI_ABORT_MESSAGE =
  "已中断：收到中断信号，已停止检测并按需切回原节点（结果不完整）。";

/** 非切换类命令被中断时：立即返回（无需切回）。 */
export function abortedEnvelope(parsed: ParsedCli): CliEnvelope<never> {
  return errEnvelope(
    parsed.command,
    "aborted",
    "已中断：收到中断信号（此命令不切换节点，无需切回）。",
    version(),
  );
}

// ───────────────────────── plumbing ─────────────────────────

async function loadConnection(
  parsed: ParsedCli,
  clientId: ClientId,
): Promise<{
  connection: ConnectionState;
  nodes: ProxyNode[];
  nodesError: string | null;
}> {
  if (parsed.mock) {
    const list = mockNodes();
    const connection: ConnectionState = {
      status: "mock",
      message: "CLI --mock 演示模式",
      config: defaultConfig(),
      currentProxy: list[0]?.name ?? null,
      usingMock: true,
      proxiesError: null,
    };
    return { connection, nodes: list, nodesError: null };
  }
  const connection: ConnectionState = {
    ...(await discoverAndProbe(undefined, clientId)),
  };
  let nodes: ProxyNode[] = [];
  let nodesError: string | null = null;
  if (connection.config && connection.status === "connected") {
    try {
      const r = await getProxies(connection.config);
      nodes = r.nodes;
      // 与 GUI useConnection 同一合并：discoverAndProbe 本身不读 /proxies，
      // currentProxy 来自节点列表（resolveCurrentProxy，规则模式下也能识别主选择组）。
      connection.currentProxy = r.currentProxy ?? connection.currentProxy;
      if (!nodes.length) {
        nodesError = r.error ?? "节点列表为空";
      }
    } catch (err) {
      nodes = [];
      nodesError = err instanceof Error ? err.message : String(err);
    }
  }
  return { connection, nodes, nodesError };
}

function collectingHooks(): {
  hooks: RunnerHooks;
  getScores: () => NodeScoreResult[];
  getCards: () => CheckCard[];
  getReport: () => EgressReport | null;
  getEnvCards: () => CheckCard[];
  getHint: () => string | null;
  getRestoreError: () => string | null;
  /** runner 在门槛之后报的失败（如节点列表为空、过程异常） */
  getLaterGateFailure: () => GateResult | null;
} {
  let scores: NodeScoreResult[] = [];
  let cards: CheckCard[] = [];
  let report: EgressReport | null = null;
  let envCards: CheckCard[] = [];
  let hint: string | null = null;
  let restoreError: string | null = null;
  let laterGateFailure: GateResult | null = null;

  const hooks: RunnerHooks = {
    onProgress: () => {},
    onNodeCards: (c) => {
      cards = c;
    },
    onUpsertNodeCard: (c) => {
      const i = cards.findIndex((x) => x.id === c.id);
      if (i >= 0) cards[i] = c;
      else cards.push(c);
    },
    onReport: (r) => {
      report = r;
    },
    onUpsertScore: (s) => {
      const i = scores.findIndex((x) => x.nodeName === s.nodeName);
      if (i >= 0) scores[i] = s;
      else scores.push(s);
    },
    onScores: (list) => {
      scores = list;
    },
    onHint: (m) => {
      hint = m;
    },
    // 单独字段：不混进 hint
    onRestoreError: (m) => {
      restoreError = m;
    },
    onGate: (g) => {
      if (!g.ok) laterGateFailure = g;
    },
    onEnvCards: (c) => {
      envCards = c;
    },
    onUpsertEnvCard: (c) => {
      const i = envCards.findIndex((x) => x.id === c.id);
      if (i >= 0) envCards[i] = c;
      else envCards.push(c);
    },
  };

  return {
    hooks,
    getScores: () => scores,
    getCards: () => cards,
    getReport: () => report,
    getEnvCards: () => envCards,
    getHint: () => hint,
    getRestoreError: () => restoreError,
    getLaterGateFailure: () => laterGateFailure,
  };
}

function cliScore(s: NodeScoreResult): CliScore {
  return { stars: s.stars, totalScore: round(s.totalScore, 1), blurb: s.blurb };
}

/** 保留 digits 位小数（去掉浮点尾巴，如 86.33333333 → 86.3）。 */
export function round(n: number, digits = 2): number {
  if (!Number.isFinite(n) || Number.isInteger(n)) return n;
  const f = 10 ** digits;
  return Math.round(n * f) / f;
}

/** 递归把 data 里所有非整数四舍五入到 2 位小数（Mbps、ms、分数等）。 */
export function roundDeep<T>(v: T, digits = 2): T {
  if (typeof v === "number") return round(v, digits) as T;
  if (Array.isArray(v)) return v.map((x) => roundDeep(x, digits)) as T;
  if (v && typeof v === "object") {
    const out: Record<string, unknown> = {};
    for (const [k, x] of Object.entries(v)) out[k] = roundDeep(x, digits);
    return out as T;
  }
  return v;
}

// ───────────────────────── 当前节点原因 ─────────────────────────

export function currentProxyReason(
  connection: Pick<ConnectionState, "status" | "currentProxy">,
  nodesError: string | null,
): string | null {
  if (connection.currentProxy) return null;
  if (connection.status !== "connected" && connection.status !== "mock") {
    return `没有连上软件（status=${connection.status}），读不到当前节点。`;
  }
  if (nodesError) return `读节点列表失败，无法判定当前节点：${nodesError}`;
  return "主选择组指向 DIRECT，或多个策略组指向不同节点、无法唯一判定当前节点。可在 Clash Verge 里切到具体节点后重试。";
}

// ───────────────────────── 筛选 ─────────────────────────

function splitList(v: string | null): string[] {
  return (v ?? "")
    .split(/[,，]/)
    .map((x) => x.trim())
    .filter(Boolean);
}

/** --region：等于/包含节点 region，或把查询词按同一规则识别成地区（HK → 中国香港）。 */
export function regionMatches(nodeRegion: string, query: string): boolean {
  const q = query.toLowerCase();
  const r = nodeRegion.toLowerCase();
  if (r === q || r.includes(q)) return true;
  const alias = detectRegion(query);
  return alias !== "其他地区" && alias === nodeRegion;
}

export function filterNodes<T extends { name: string; region: string }>(
  nodes: T[],
  filter: NodeFilter,
): T[] {
  const regions = splitList(filter.region);
  const words = splitList(filter.match).map((w) => w.toLowerCase());
  let out = nodes.filter(
    (n) =>
      (!regions.length || regions.some((q) => regionMatches(n.region, q))) &&
      (!words.length || words.some((w) => n.name.toLowerCase().includes(w))),
  );
  if (filter.limit != null) out = out.slice(0, filter.limit);
  return out;
}

export function hasFilter(f: NodeFilter): boolean {
  return !!(f.region || f.match || f.limit != null);
}

const EMPTY_FILTER: NodeFilter = { region: null, match: null, limit: null };

// ───────────────────────── 进度（NDJSON → stderr） ─────────────────────────

export type CliProgressStage =
  | "connect"
  | "gate"
  | "precheck"
  | "switch"
  | "test"
  | "restore"
  | "probe";

/** 一行一个对象，写到 stderr。字段稳定，见 docs/CLI.md「进度事件」。 */
export type CliProgressEvent = {
  type: "start" | "stage" | "card" | "node_done" | "done";
  command: string;
  ts: string;
  stage?: CliProgressStage;
  /** 1-based；预检阶段为已完成数 */
  index?: number;
  total?: number;
  nodeName?: string;
  text?: string;
  /** card：卡片 id 与等级 */
  cardId?: string;
  level?: string;
  /** node_done：该节点评分 */
  stars?: NodeStars;
  totalScore?: number;
  /** done */
  ok?: boolean;
};

export type CliOsInfo = { os: string; version: string | null; major: number | null };

/** CLI 隐藏窗跑深测所需最低 macOS 主版本（关闭 WebView 后台节流需要 14+）。 */
export const CLI_MIN_MACOS = 14;

export type ShimInstaller = (dir: string | null) => Promise<unknown>;

export type CliDispatchOptions = {
  /** 中断信号（Ctrl+C / SIGTERM，经 Rust 转来）。check all 会在节点间停止并切回。 */
  shouldAbort?: () => boolean;
  /** 进度事件（bootCli 写 stderr NDJSON）；不给则不发 */
  onProgress?: (ev: CliProgressEvent) => void;
  /** 系统信息（bootCli 从 Rust 取）；macOS < 14 时 check/env 直接 os_unsupported */
  osInfo?: CliOsInfo | null;
  /** install-shim 的实现（bootCli 接 Rust）；不给则 shim_unavailable */
  installShim?: ShimInstaller;
};

/** 对外入口：调度 + 数字取整 + 进度 start/done。 */
export async function dispatchCli(
  parsed: ParsedCli,
  opts: CliDispatchOptions = {},
): Promise<CliEnvelope> {
  const label = parsed.parseError ? (parsed.commandToken ?? parsed.command) : parsed.command;
  const emit = progressEmitter(label, opts.onProgress);
  const long = isLongCommand(parsed);
  if (long) emit({ type: "start", text: describeCommand(parsed) });
  const env = await dispatchInner(parsed, opts, emit);
  if (env.data !== undefined) env.data = roundDeep(env.data);
  if (long) emit({ type: "done", ok: env.ok });
  return env;
}

type Emit = (ev: Omit<CliProgressEvent, "command" | "ts">) => void;

function progressEmitter(command: string, sink?: (ev: CliProgressEvent) => void): Emit {
  return (ev) => {
    if (!sink) return;
    try {
      sink({ ...ev, command, ts: new Date().toISOString() } as CliProgressEvent);
    } catch {
      // 进度只是旁路信息，失败不影响结果
    }
  };
}

/** 会发进度事件的命令：check（任意目标）、env。 */
export function isLongCommand(parsed: ParsedCli): boolean {
  return !parsed.parseError && (parsed.command === "check" || parsed.command === "env");
}

function describeCommand(parsed: ParsedCli): string {
  if (parsed.command !== "check") return parsed.command;
  const t = parsed.checkTarget;
  if (!t || t.kind === "current") return "check current";
  if (t.kind === "node") return `check node ${t.name}`;
  return t.names?.length ? `check nodes ${t.names.join(" ")}` : "check all";
}

/** 把 runner 进度 / 卡片 / 评分转成 CLI 进度事件。 */
function progressHooks(emit: Emit, bag: ReturnType<typeof collectingHooks>, nodeName?: () => string | undefined): void {
  const h = bag.hooks;
  let lastTotal: number | undefined;
  let lastIndex: number | undefined;
  let lastNode: string | undefined;
  h.onProgress = (p) => {
    if (!p) return;
    if (p.stage === "test") {
      lastTotal = p.total;
      lastIndex = p.current;
      lastNode = p.testingNode;
    }
    emit({
      type: "stage",
      stage: p.stage ?? "test",
      index: p.current,
      total: p.total,
      nodeName: p.testingNode,
      text: p.text,
    });
  };
  const upsertCard = h.onUpsertNodeCard;
  h.onUpsertNodeCard = (c) => {
    upsertCard?.(c);
    if (c.level === "running") return;
    emit({ type: "card", cardId: c.id, level: c.level, nodeName: lastNode ?? nodeName?.() });
  };
  const upsertEnv = h.onUpsertEnvCard;
  h.onUpsertEnvCard = (c) => {
    upsertEnv?.(c);
    if (c.level === "running") return;
    emit({ type: "card", cardId: c.id, level: c.level });
  };
  const upsertScore = h.onUpsertScore;
  h.onUpsertScore = (sc) => {
    upsertScore?.(sc);
    const tested = sc.nodeName === lastNode;
    emit({
      type: "node_done",
      nodeName: sc.nodeName,
      index: tested ? lastIndex : undefined,
      total: tested ? lastTotal : undefined,
      stars: sc.stars,
      totalScore: round(sc.totalScore, 1),
    });
  };
}

async function dispatchInner(
  parsed: ParsedCli,
  opts: CliDispatchOptions,
  emit: Emit,
): Promise<CliEnvelope> {
  const ver = version();
  const cmd = parsed.command;
  const isAborted = () => opts.shouldAbort?.() === true;

  if (parsed.parseError) {
    const label =
      parsed.parseError.code === "unknown_command"
        ? (parsed.commandToken ?? "unknown")
        : cmd;
    return errEnvelope(
      label,
      parsed.parseError.code,
      parsed.parseError.message,
      ver,
    );
  }

  if (cmd === "help") {
    return okEnvelope("help", { text: CLI_HELP_TEXT, spec: CLI_SPEC }, ver);
  }

  if (cmd === "serve") {
    // 长驻由 bootCli → bootServe；若误入 dispatch 则明确报错（不静默）。
    return errEnvelope(
      "serve",
      "serve_not_oneshot",
      "serve 是常驻 HTTP 服务，应由 CLI 启动路径接管，不能当一次性命令调度。",
      ver,
    );
  }

  if (cmd === "install-shim") {
    if (!opts.installShim) {
      return errEnvelope(
        cmd,
        "shim_unavailable",
        "install-shim 只能在应用二进制里运行。",
        ver,
      );
    }
    try {
      const r = (await opts.installShim(parsed.dir)) as Record<string, unknown>;
      const onPath = r.onPath === true;
      const dir = String(r.dir ?? "");
      return okEnvelope(
        cmd,
        {
          ...r,
          appBinary: CLI_APP_BINARY,
          next: onPath
            ? "现在可以直接运行：egress-checker discover --client verge"
            : `${dir} 不在 PATH 里：把 export PATH="${dir}:$PATH" 加到 ~/.zshrc 后重开终端；或直接用 ${r.path ?? dir}`,
          warning:
            r.targetIsDevBuild === true
              ? "shim 指向的是开发构建（target/debug|release）。正式使用请从 /Applications 里的应用重新运行 install-shim。"
              : null,
        },
        ver,
      );
    } catch (err) {
      const e = err as { code?: unknown; message?: unknown };
      const code = typeof e?.code === "string" ? e.code : "shim_failed";
      const message =
        typeof e?.message === "string" ? e.message : err instanceof Error ? err.message : String(err);
      return errEnvelope(cmd, code, message, ver);
    }
  }

  // macOS < 14：隐藏窗 WebView 会被挂起，check/env 会永远卡住 → 立即失败。
  if ((cmd === "check" || cmd === "env") && opts.osInfo?.os === "macos") {
    const major = opts.osInfo.major;
    if (major != null && major < CLI_MIN_MACOS) {
      return errEnvelope(
        cmd,
        "os_unsupported",
        `当前 macOS ${opts.osInfo.version ?? major} 低于 ${CLI_MIN_MACOS}：CLI 隐藏窗里无法关闭 WebView 后台节流，${cmd} 会卡住，已拒绝运行。`,
        ver,
      );
    }
  }

  const client = resolveCliClient(parsed);
  if (!client.ok) {
    return errEnvelope(cmd, client.error.code, client.error.message, ver);
  }
  const clientId = client.clientId;

  try {
    if (cmd === "discover") {
      const { connection, nodes, nodesError } = await loadConnection(
        parsed,
        clientId,
      );
      const data: DiscoverData = {
        clientId,
        status: connection.status,
        message: connection.message,
        currentProxy: connection.currentProxy,
        currentProxyReason: currentProxyReason(connection, nodesError),
        usingMock: connection.usingMock,
        config: connection.config
          ? {
              host: connection.config.host,
              port: connection.config.port,
              mixedPort: connection.config.mixedPort,
              source: connection.config.source,
              sockPath: connection.config.sockPath ?? null,
              // never echo secret
            }
          : null,
        nodeCount: nodes.length,
        nodesTruncated: nodes.length > CLI_NODES_LIMIT,
        nodesError,
        nodes: nodes.slice(0, CLI_NODES_LIMIT).map((n) => ({
          name: n.name,
          type: n.type,
          region: n.region,
        })),
      };
      return okEnvelope("discover", data, ver);
    }

    if (cmd === "nodes") {
      const { connection, nodes, nodesError } = await loadConnection(parsed, clientId);
      const picked = filterNodes(nodes, parsed.filter);
      const data: NodesData = {
        clientId,
        status: connection.status,
        currentProxy: connection.currentProxy,
        total: nodes.length,
        count: picked.length,
        filter: parsed.filter,
        nodesError,
        nodes: picked.map((n) => ({
          name: n.name,
          type: n.type,
          region: n.region,
          current: n.name === connection.currentProxy,
          lastDelay: n.lastDelay ?? null,
        })),
      };
      return okEnvelope("nodes", data, ver);
    }

    if (cmd === "gate") {
      const { connection } = await loadConnection(parsed, clientId);
      const gate = await runLightGate(connection);
      const data: GateData = {
        clientId,
        connectionStatus: connection.status,
        gate,
      };
      return okEnvelope("gate", data, ver);
    }

    if (cmd === "env") {
      emit({ type: "stage", stage: "connect", text: "连接 Clash Verge" });
      const { connection } = await loadConnection(parsed, clientId);
      const bag = collectingHooks();
      progressHooks(emit, bag);
      emit({ type: "stage", stage: "probe", text: "环境泄漏检查" });
      await runEnv(
        {
          connection,
          mixedPort: connection.config?.mixedPort ?? null,
          exitIp: null,
        },
        bag.hooks,
      );
      const data: EnvData = { clientId, cards: bag.getEnvCards() };
      return okEnvelope("env", data, ver);
    }

    if (cmd === "check") {
      const target = parsed.checkTarget ?? { kind: "current" as const };
      emit({ type: "stage", stage: "connect", text: "连接 Clash Verge、读取节点" });
      const { connection, nodes, nodesError } = await loadConnection(
        parsed,
        clientId,
      );
      const mixedPort = connection.config?.mixedPort ?? null;
      const bag = collectingHooks();
      const currentName = connection.currentProxy ?? "当前节点";
      progressHooks(emit, bag, () =>
        target.kind === "current" ? currentName : undefined,
      );

      if (target.kind === "current") {
        emit({
          type: "stage",
          stage: "test",
          index: 1,
          total: 1,
          nodeName: currentName,
          text: `正在检测 ${currentName}…`,
        });
        const report = await runNodeDiagnostics(bag.hooks.onUpsertNodeCard, {
          mixedPort,
          mihomoConfig: connection.config,
        });
        const name = connection.currentProxy ?? "当前节点";
        const score = scoreNodeFromCards(name, report.cards, report.ranAt);
        const data: CheckCurrentData = {
          target: "current",
          clientId,
          nodeName: name,
          score: cliScore(score),
          cards: report.cards,
          note: report.note,
        };
        return okEnvelope("check", data, ver);
      }

      // node / all 先过门槛：不过就不切换、不深测。
      // 筛选在门槛之前做：参数错了不必跑网络。
      let selected: ProxyNode[] | null = null;
      if (target.kind === "all" && (target.names?.length || hasFilter(parsed.filter))) {
        let pool = nodes;
        if (target.names?.length) {
          const picked: ProxyNode[] = [];
          for (const name of target.names) {
            const found = resolveCliNode(nodes, name, nodesError);
            if (!found.ok) {
              return errEnvelope("check", found.error.code, found.error.message, ver);
            }
            if (!picked.includes(found.node)) picked.push(found.node);
          }
          pool = picked;
        }
        selected = filterNodes(pool, parsed.filter);
        if (!selected.length) {
          return errEnvelope(
            "check",
            nodes.length ? "no_nodes_matched" : "nodes_unavailable",
            nodes.length
              ? `筛选后没有节点（共 ${nodes.length} 个；region=${parsed.filter.region ?? "-"} match=${parsed.filter.match ?? "-"}）。`
              : `没有读到节点列表。${nodesError ? `原因：${nodesError}` : ""}`,
            ver,
          );
        }
      }
      emit({ type: "stage", stage: "gate", text: "轻量门槛" });
      const gate = await runLightGate(connection);
      if (!gate.ok) {
        return errEnvelope("check", "gate_failed", gate.message, ver, {
          target: target.kind,
          clientId,
          gate,
        });
      }
      const ensureGate = async () => gate;

      if (target.kind === "node") {
        // parse 已保证 name 非空；这里再守一次，防止直接调用 dispatchCli。
        if (!target.name) {
          return errEnvelope(
            "check",
            "node_name_required",
            'check node 需要节点名称，例如：--cli check node "香港"',
            ver,
          );
        }
        const found = resolveCliNode(nodes, target.name, nodesError);
        if (!found.ok) {
          return errEnvelope("check", found.error.code, found.error.message, ver);
        }
        const node = found.node;
        const finished = await testOne(
          {
            node,
            connection,
            forceMock: parsed.mock,
            mixedPort,
            ensureGate,
          },
          bag.hooks,
        );
        const score =
          bag.getScores().find((s) => s.nodeName === node.name) ?? null;
        const data: CheckNodeData = {
          target: "node",
          clientId,
          nodeName: node.name,
          score: score ? cliScore(score) : null,
          cards: score?.cards ?? bag.getCards(),
          hint: bag.getHint(),
          restoreError: bag.getRestoreError(),
          aborted: isAborted(),
          gate,
        };
        if (data.restoreError) {
          return errEnvelope("check", "restore_failed", data.restoreError, ver, data);
        }
        if (data.aborted) {
          return errEnvelope("check", "aborted", CLI_ABORT_MESSAGE, ver, data);
        }
        if (!finished || !score) {
          const later = bag.getLaterGateFailure();
          return errEnvelope(
            "check",
            "check_failed",
            data.hint ?? later?.message ?? "检测没有完成。",
            ver,
            data,
          );
        }
        return okEnvelope("check", data, ver);
      }

      // all
      const finished = await testAll(
        {
          connection,
          nodes: selected ?? nodes,
          forceMock: parsed.mock,
          mixedPort,
          ensureGate,
          shouldAbort: isAborted,
        },
        bag.hooks,
      );
      const scores = bag.getScores();
      const data: CheckAllData = {
        target: "all",
        clientId,
        count: scores.length,
        selection: selected
          ? {
              filter: target.kind === "all" ? parsed.filter : EMPTY_FILTER,
              names: target.kind === "all" && target.names?.length ? target.names : null,
              selected: selected.map((n) => n.name),
            }
          : null,
        scores: scores.map((s) => ({ nodeName: s.nodeName, ...cliScore(s) })),
        hint: bag.getHint(),
        restoreError: bag.getRestoreError(),
        aborted: isAborted(),
        nodesError,
        gate,
      };
      if (data.restoreError) {
        return errEnvelope("check", "restore_failed", data.restoreError, ver, data);
      }
      if (data.aborted) {
        return errEnvelope("check", "aborted", CLI_ABORT_MESSAGE, ver, data);
      }
      const later = bag.getLaterGateFailure();
      if (!finished || later) {
        return errEnvelope(
          "check",
          "check_failed",
          later?.message ?? data.hint ?? "测全部节点没有完成。",
          ver,
          data,
        );
      }
      return okEnvelope("check", data, ver);
    }

    // CliCommandName 已穷举；到这里说明类型与实现不同步。
    const never: never = cmd;
    return errEnvelope(String(never), "unknown_command", `未知命令：${String(never)}`, ver);
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    return errEnvelope(cmd, "dispatch_error", message, ver);
  }
}

// ───────────────────────── --no-json 人类摘要 ─────────────────────────

function starsText(s: NodeStars): string {
  return s === "unavailable" ? "不可用" : `★${s}`;
}

const LEVEL_TEXT: Record<CheckCard["level"], string> = {
  pass: "通过",
  warn: "注意",
  fail: "失败",
  unknown: "未知",
  running: "进行中",
};

function cardLines(cards: CheckCard[]): string[] {
  return cards.map((c) => `  [${LEVEL_TEXT[c.level] ?? c.level}] ${c.title}：${c.conclusion}`);
}

function isObj(v: unknown): v is Record<string, unknown> {
  return typeof v === "object" && v !== null;
}

function dataLines(command: string, data: unknown): string[] {
  if (!isObj(data)) return [];
  const out: string[] = [];
  if (command === "discover" && "nodeCount" in data) {
    const d = data as DiscoverData;
    out.push(`  软件：${clientLabel(d.clientId)}`);
    out.push(`  状态：${d.status}（${d.message}）`);
    out.push(`  当前节点：${d.currentProxy ?? `未知（${d.currentProxyReason ?? ""}）`}`);
    out.push(
      `  节点：${d.nodeCount} 个${d.nodesTruncated ? `（JSON 只列前 ${CLI_NODES_LIMIT} 个；全部请用 nodes 命令）` : ""}`,
    );
    if (d.nodesError) out.push(`  读节点失败：${d.nodesError}`);
    return out;
  }
  if (command === "nodes" && "total" in data) {
    const d = data as NodesData;
    out.push(`  节点：${d.count} / ${d.total} 个；当前：${d.currentProxy ?? "未知"}`);
    for (const n of d.nodes) out.push(`  ${n.current ? "●" : " "} ${n.name}（${n.region}）`);
    if (d.nodesError) out.push(`  读节点失败：${d.nodesError}`);
    return out;
  }
  if (command === "install-shim" && "path" in data) {
    const d = data as { path: string; next?: string; warning?: string | null };
    out.push(`  已写入：${d.path}`);
    if (d.next) out.push(`  ${d.next}`);
    if (d.warning) out.push(`  ⚠ ${d.warning}`);
    return out;
  }
  if (command === "gate" && "gate" in data && !("target" in data)) {
    const d = data as GateData;
    out.push(`  门槛：${d.gate.ok ? "通过" : "未通过"} — ${d.gate.message}`);
    return out;
  }
  if (command === "env" && "cards" in data) {
    const d = data as EnvData;
    out.push(`  共 ${d.cards.length} 项`);
    out.push(...cardLines(d.cards));
    return out;
  }
  if (command === "check" && "target" in data) {
    // gate_failed 时 data 只有 { target, clientId, gate }
    if (!("nodeName" in data) && !("scores" in data)) {
      const g = (data as { gate?: GateResult }).gate;
      if (g) out.push(`  门槛：${g.ok ? "通过" : "未通过"} — ${g.message}`);
      return out;
    }
    if (data.target === "current" || data.target === "node") {
      const d = data as CheckCurrentData | CheckNodeData;
      out.push(`  节点：${d.nodeName}`);
      if (d.score) {
        out.push(
          `  评分：${starsText(d.score.stars)} · ${d.score.totalScore} 分 · ${d.score.blurb}`,
        );
      }
      out.push(...cardLines(d.cards));
      if (d.target === "node") {
        if (d.hint) out.push(`  提示：${d.hint}`);
        if (d.restoreError) {
          out.push(`  ⚠ ${d.restoreError}请到代理软件里手动选回。`);
        }
      }
      return out;
    }
    if (data.target === "all") {
      const d = data as CheckAllData;
      if (d.selection) out.push(`  本次选中：${d.selection.selected.join("、")}`);
      out.push(`  共 ${d.count} 个节点${d.aborted ? "（已中断，结果不完整）" : ""}`);
      d.scores.forEach((s, i) => {
        out.push(
          `  ${String(i + 1).padStart(2)}. ${starsText(s.stars).padEnd(5)} ${String(s.totalScore).padStart(3)} 分  ${s.nodeName} — ${s.blurb}`,
        );
      });
      if (d.nodesError) out.push(`  读节点失败：${d.nodesError}`);
      if (d.hint) out.push(`  提示：${d.hint}`);
      if (d.restoreError) out.push(`  ⚠ ${d.restoreError}请到代理软件里手动选回。`);
      return out;
    }
  }
  return out;
}

function headLabel(env: CliEnvelope): string {
  if (env.command === "check" && isObj(env.data) && typeof env.data.target === "string") {
    return `check ${env.data.target}`;
  }
  return env.command;
}

/** 人类可读摘要（--no-json）。help 打印纯文本；其它命令首行 ✓/✗ + 要点。 */
export function formatCliHuman(env: CliEnvelope): string {
  if (env.ok && env.command === "help") {
    const text = (env.data as { text?: string } | undefined)?.text;
    return text ?? CLI_HELP_TEXT;
  }
  const lines: string[] = [];
  if (env.ok) {
    lines.push(`✓ ${headLabel(env)} 完成`);
  } else {
    lines.push(
      `✗ ${headLabel(env)} 失败 [${env.error?.code ?? "?"}] ${env.error?.message ?? ""}`.trimEnd(),
    );
  }
  if (!env.ok && env.error?.hint) lines.push(`  建议：${env.error.hint}`);
  lines.push(...dataLines(env.command, env.data));
  return lines.join("\n");
}
