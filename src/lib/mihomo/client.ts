/**
 * Mihomo REST client.
 * Prefer Tauri Rust HTTP (reqwest) to 127.0.0.1 to avoid WebView CORS;
 * fall back to unix socket invoke. Browser fetch is dev-only by design
 * (production never sends the secret over plaintext HTTP from the WebView).
 * Never log secret values.
 * Never auto-fallback to mock nodes — Mock is Settings toggle only.
 */

import { invoke } from "@tauri-apps/api/core";
import {
  IGNORE_PROXY_TYPES,
  JUNK_NAME_KEYWORDS,
  type ConnectionState,
  type ControllerConfig,
  type ProxyInfo,
  type ProxyNode,
} from "./types";
import { detectRegion, detectRegions } from "./regions";
import {
  resolveCurrentProxy,
  resolveCurrentProxyFromSlim,
  resolveSelectorSnapshotFromMap,
  resolveSwitchPlanFromMap,
  type SlimGroupLike,
  type SelectorSnapshot,
} from "./currentProxy";
export type { SelectorSnapshot };
import { mockProxiesRaw } from "./mock";
import { mergeProviderNodes, missingProviderNames, resolveProviderNode, type ProviderMap } from "./providerNodes";
import {
  type ClientId,
  clientLabel,
  clientPreset,
  clientUnreachableHint,
  vergeLikeDefault,
} from "./clients";

const isTauri = () =>
  typeof window !== "undefined" &&
  ("__TAURI_INTERNALS__" in window || "__TAURI__" in window);

async function discoverViaRust(): Promise<ControllerConfig | null> {
  if (!isTauri()) return null;
  try {
    return await invoke<ControllerConfig>("discover_mihomo");
  } catch {
    return null;
  }
}

async function discoverViaRustForClient(
  clientId: ClientId,
): Promise<ControllerConfig | null> {
  if (!isTauri()) return null;
  try {
    return await invoke<ControllerConfig>("discover_mihomo_for_client", {
      clientId,
    });
  } catch {
    return null;
  }
}

export function defaultConfig(): ControllerConfig {
  return vergeLikeDefault();
}

type HttpResult = { status: number; json: unknown; raw: string };

function is2xx(status: number): boolean {
  return status >= 200 && status < 300;
}

async function rustHttp(
  config: ControllerConfig,
  method: string,
  path: string,
  body?: string,
  timeoutMs = 3000,
): Promise<HttpResult | null> {
  if (!isTauri()) return null;
  try {
    const res = await invoke<{ status: number; body: string }>("mihomo_http", {
      req: {
        host: config.host,
        port: config.port,
        method,
        path,
        body: body ?? null,
        secret: config.secret,
        timeoutMs,
      },
    });
    let json: unknown = null;
    try {
      json = res.body ? JSON.parse(res.body) : null;
    } catch {
      json = null;
    }
    return { status: res.status, json, raw: res.body };
  } catch {
    return null;
  }
}

async function unixHttp(
  config: ControllerConfig,
  method: string,
  path: string,
  body?: string,
  timeoutMs = 3000,
): Promise<HttpResult | null> {
  if (!isTauri()) return null;
  try {
    const res = await invoke<{ status: number; body: string }>("mihomo_unix_http", {
      req: {
        method,
        path,
        body: body ?? null,
        secret: config.secret,
        sockPath: config.sockPath ?? null,
        timeoutMs,
      },
    });
    let json: unknown = null;
    try {
      json = res.body ? JSON.parse(res.body) : null;
    } catch {
      json = null;
    }
    return { status: res.status, json, raw: res.body };
  } catch {
    return null;
  }
}

async function browserFetch(
  config: ControllerConfig,
  method: string,
  path: string,
  body?: unknown,
  timeoutMs = 3000,
): Promise<HttpResult | null> {
  // Dev-only transport: sends the controller secret as plaintext-HTTP Bearer
  // from the WebView. Production must go through Rust (mihomo_http / mihomo_unix_http).
  if (!import.meta.env.DEV) return null;
  const url = `http://${config.host}:${config.port}${path}`;
  const headers: Record<string, string> = {
    "Content-Type": "application/json",
  };
  if (config.secret) {
    headers.Authorization = `Bearer ${config.secret}`;
  }

  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), timeoutMs);
  try {
    const res = await fetch(url, {
      method,
      headers,
      body: body !== undefined ? JSON.stringify(body) : undefined,
      signal: controller.signal,
    });
    const raw = await res.text();
    let json: unknown = null;
    try {
      json = raw ? JSON.parse(raw) : null;
    } catch {
      json = null;
    }
    return { status: res.status, json, raw };
  } catch {
    return null;
  } finally {
    clearTimeout(timer);
  }
}

/**
 * Prefer first successful 2xx.
 * When sockPath is set (Verge 2.5.6+ / Party), try unix before TCP — EC is often blank.
 * Otherwise TCP → unix → browser. Do not return a failed TCP as final if unix might work.
 */
async function httpApi(
  config: ControllerConfig,
  method: string,
  path: string,
  body?: unknown,
  timeoutMs = 3000,
): Promise<HttpResult | null> {
  const bodyStr = body !== undefined ? JSON.stringify(body) : undefined;
  const preferSock = !!config.sockPath;

  let viaRust: HttpResult | null = null;
  let viaSock: HttpResult | null = null;

  if (preferSock) {
    viaSock = await unixHttp(config, method, path, bodyStr, timeoutMs);
    if (viaSock && is2xx(viaSock.status)) return viaSock;
    viaRust = await rustHttp(config, method, path, bodyStr, timeoutMs);
    if (viaRust && is2xx(viaRust.status)) return viaRust;
  } else {
    viaRust = await rustHttp(config, method, path, bodyStr, timeoutMs);
    if (viaRust && is2xx(viaRust.status)) return viaRust;
    viaSock = await unixHttp(config, method, path, bodyStr, timeoutMs);
    if (viaSock && is2xx(viaSock.status)) return viaSock;
  }

  const viaBrowser = await browserFetch(config, method, path, body, timeoutMs);
  if (viaBrowser && is2xx(viaBrowser.status)) return viaBrowser;

  // No 2xx: prefer any non-null response (sock > tcp > browser) so callers see 401 etc.
  return viaSock ?? viaRust ?? viaBrowser;
}

type ControllerReader = (path: string, timeoutMs: number) => Promise<HttpResult | null>;

/** Pick one responding controller and retain it for related discovery requests.
 * An HTTP error is a response, not permission to borrow another controller's
 * nodes. In Tauri, controller credentials stay in native transports. */
async function openControllerRead(
  config: ControllerConfig, path: string, timeoutMs: number,
): Promise<{ response: HttpResult; read: ControllerReader } | null> {
  const tcp: ControllerReader = (next, timeout) => rustHttp(config, "GET", next, undefined, timeout);
  const unix: ControllerReader = (next, timeout) => unixHttp(config, "GET", next, undefined, timeout);
  const browser: ControllerReader = (next, timeout) => browserFetch(config, "GET", next, undefined, timeout);
  const readers = isTauri() ? (config.sockPath ? [unix, tcp] : [tcp, unix]) : [browser];
  for (const read of readers) {
    const response = await read(path, timeoutMs);
    if (response) return { response, read };
  }
  return null;
}

function isJunkName(name: string): boolean {
  if (name.startsWith("PASS") || name.startsWith("REJECT")) return true;
  return JUNK_NAME_KEYWORDS.some((k) => name.includes(k));
}

function toNode(name: string, p: ProxyInfo, region?: string): ProxyNode {
  return {
    name,
    type: p.type,
    region: region ?? detectRegion(name),
    udp: p.udp,
    xudp: p.xudp,
    uot: p.uot,
    tfo: p.tfo,
    smux: p.smux,
    mptcp: p.mptcp,
    alive: p.alive,
    lastDelay: p.history?.length
      ? p.history[p.history.length - 1].delay
      : undefined,
    lastDelayAt: p.history?.length
      ? p.history[p.history.length - 1].time
      : undefined,
    raw: p,
  };
}

export function filterNodes(
  proxies: Record<string, ProxyInfo>,
): { nodes: ProxyNode[]; currentProxy: string | null } {
  const currentProxy = resolveCurrentProxy(proxies);
  const entries = Object.entries(proxies).filter(
    ([name, p]) => !IGNORE_PROXY_TYPES.has(p.type) && !isJunkName(name),
  );
  const regions = detectRegions(entries.map(([name]) => name));
  const nodes: ProxyNode[] = entries.map(([name, p], i) =>
    toNode(name, p, regions[i]),
  );
  return { nodes, currentProxy };
}

/**
 * When flat filter yields empty, resolve leaf names from Selector `all` arrays
 * that exist as keys in the proxies map (common Clash profile shape).
 */
export function resolveNodesFromGroups(
  proxies: Record<string, ProxyInfo>,
): { nodes: ProxyNode[]; currentProxy: string | null } {
  const currentProxy = resolveCurrentProxy(proxies);

  const leafNames = new Set<string>();
  const preferGroups = ["Proxy", "GLOBAL", "proxy"];

  const considerAll = (all: string[] | undefined) => {
    if (!all) return;
    for (const name of all) {
      const p = proxies[name];
      if (!p) continue;
      if (IGNORE_PROXY_TYPES.has(p.type)) continue;
      if (isJunkName(name)) continue;
      leafNames.add(name);
    }
  };

  for (const g of preferGroups) {
    considerAll(proxies[g]?.all);
  }
  for (const [, p] of Object.entries(proxies)) {
    if (p.type === "Selector" || p.type === "URLTest" || p.type === "Fallback") {
      considerAll(p.all);
    }
  }

  const nodes = [...leafNames].map((name) => toNode(name, proxies[name]));
  return { nodes, currentProxy };
}

export async function discoverAndProbe(
  manual?: Partial<ControllerConfig>,
  clientId: ClientId = "verge",
): Promise<ConnectionState> {
  const label = clientLabel(clientId);
  const preset = clientPreset(clientId);
  let config: ControllerConfig = { ...defaultConfig(), ...preset };

  // Auto-discover per selected client — never silently use Verge sock for others.
  if (clientId === "verge") {
    const discovered = await discoverViaRust();
    if (discovered) {
      config = { ...config, ...discovered };
    }
  } else {
    const discovered = await discoverViaRustForClient(clientId);
    if (discovered) {
      config = { ...config, ...discovered };
    }
    // Hard-clear any Verge sock that presets/defaults might have left behind.
    if (clientId !== "mihomo_party") {
      config = { ...config, sockPath: null };
    } else if (
      config.sockPath &&
      config.sockPath.includes("verge-mihomo")
    ) {
      config = {
        ...config,
        sockPath: "/tmp/mihomo-party.sock",
      };
    }
  }

  // Optional advanced override from Settings (never required for main path).
  if (manual && Object.keys(manual).length > 0) {
    const src = manual.source;
    // Ignore "auto" reset sentinel — discovery already ran.
    if (src !== "auto") {
      config = mergeManual(config, manual);
    }
    // After merge, still never keep Verge sock for non-Verge unless user set sockPath.
    if (clientId !== "verge" && !manual.sockPath) {
      if (clientId === "mihomo_party") {
        const party = "/tmp/mihomo-party.sock";
        config = {
          ...config,
          sockPath: config.sockPath?.includes("verge") ? party : (config.sockPath ?? party),
        };
      } else {
        config = { ...config, sockPath: null };
      }
    }
  }

  // mihomo_party: prefer unix sock first when present
  if (clientId === "mihomo_party") {
    const partySock = config.sockPath || "/tmp/mihomo-party.sock";
    config = { ...config, sockPath: partySock };
    const sockFirst = await probeWithConfig(config, clientId, { preferSock: true });
    if (sockFirst.status === "connected" || sockFirst.status === "unauthorized") {
      return sockFirst;
    }
    // Then TCP if any
    const tcp = await probeWithConfig({ ...config }, clientId, { preferSock: false });
    if (tcp.status === "connected" || tcp.status === "unauthorized") {
      return tcp;
    }
    return {
      ...sockFirst,
      message: clientUnreachableHint(clientId, config.source),
      proxiesError: "请确认 Mihomo Party 已打开并已连接节点",
    };
  }

  // Verge: prefer unix sock first (2.5.6+ service sock; TCP EC often blank/stale).
  if (clientId === "verge" && config.sockPath) {
    const sockFirst = await probeWithConfig(config, clientId, { preferSock: true });
    if (sockFirst.status === "connected" || sockFirst.status === "unauthorized") {
      return sockFirst;
    }
    const tcp = await probeWithConfig({ ...config }, clientId, { preferSock: false });
    if (tcp.status === "connected" || tcp.status === "unauthorized") {
      return tcp;
    }
    return {
      ...sockFirst,
      message: clientUnreachableHint(clientId, config.source),
      proxiesError: `请确认【${label}】已打开并已连接节点`,
    };
  }

  const result = await probeWithConfig(config, clientId);
  if (result.status === "connected" || result.status === "unauthorized") {
    return result;
  }

  // Nyanpasu debug builds may listen on 9872 instead of default 17650.
  if (clientId === "nyanpasu" && config.port !== 9872 && !manual?.port) {
    const alt = await probeWithConfig(
      { ...config, port: 9872, source: `${config.source}+port-9872` },
      clientId,
    );
    if (alt.status === "connected" || alt.status === "unauthorized") {
      return alt;
    }
  }

  return {
    ...result,
    message: clientUnreachableHint(clientId, config.source),
    proxiesError:
      clientId === "flclash"
        ? clientUnreachableHint("flclash", config.source)
        : `请确认【${label}】已打开并已连接节点`,
  };
}

function mergeManual(
  config: ControllerConfig,
  manual: Partial<ControllerConfig>,
): ControllerConfig {
  const isManual =
    manual.source === "manual" ||
    manual.host !== undefined ||
    manual.port !== undefined ||
    manual.secret !== undefined ||
    manual.mixedPort !== undefined;
  return {
    ...config,
    ...manual,
    source:
      isManual && manual.source !== "auto"
        ? (manual.source ?? "manual")
        : config.source,
  };
}

/**
 * Best-effort: query live `GET /configs` from the connected Mihomo controller
 * to sync the real-time `mixed-port` (or `port` fallback). This guarantees
 * non-TUN (System Proxy / port-only) users and FlClash users with dynamic port
 * overrides always probe through the actual listening proxy port.
 */
function isLoopbackExempted(skipPrefixes: unknown): boolean {
  if (!Array.isArray(skipPrefixes)) return false;
  return skipPrefixes.some((entry) => {
    if (typeof entry !== "string") return false;
    const p = entry.trim();
    return (
      p === "127.0.0.1" ||
      p.startsWith("127.0.0.1/") ||
      p.startsWith("127.0.0.0/")
    );
  });
}

async function syncLiveMixedPort(config: ControllerConfig): Promise<ControllerConfig> {
  try {
    const res = await httpApi(config, "GET", "/configs", undefined, 1500);
    if (res && is2xx(res.status) && res.json && typeof res.json === "object") {
      const obj = res.json as Record<string, unknown>;
      const mp = obj["mixed-port"];
      const hp = obj["port"];
      const authList = obj["authentication"];
      const hasAuth =
        Array.isArray(authList) &&
        authList.some((item) => typeof item === "string" && item.trim().length > 0);
      const inboundAuth = hasAuth && !isLoopbackExempted(obj["skip-auth-prefixes"]);

      let next: ControllerConfig = inboundAuth
        ? { ...config, inboundAuth: true }
        : config;

      if (typeof mp === "number" && mp > 0 && mp <= 65535) {
        return { ...next, mixedPort: mp, mixedPortDisabled: false };
      }
      if (typeof hp === "number" && hp > 0 && hp <= 65535 && (!mp || mp === 0)) {
        return { ...next, mixedPort: hp, mixedPortDisabled: false };
      }
      if (mp === 0 && (hp === 0 || hp === undefined)) {
        return { ...next, mixedPort: 0, mixedPortDisabled: true };
      }
      return next;
    }
  } catch {
    /* ignore and keep discovered mixedPort */
  }
  return config;
}

async function connectedState(
  config: ControllerConfig,
  label: string,
): Promise<ConnectionState> {
  const synced = await syncLiveMixedPort(config);
  return {
    status: "connected",
    message: `已连上 ${label}`,
    config: synced,
    currentProxy: null,
    usingMock: false,
    proxiesError: null,
  };
}

async function probeWithConfig(
  config: ControllerConfig,
  clientId?: ClientId,
  opts?: { preferSock?: boolean },
): Promise<ConnectionState> {
  const trySock = !!config.sockPath;
  const label = clientId ? clientLabel(clientId) : "客户端";
  const preferSock = opts?.preferSock === true && trySock;

  const runTcp = async () => rustHttp(config, "GET", "/version", undefined, 1500);
  const runSock = async () =>
    trySock ? unixHttp(config, "GET", "/version", undefined, 2000) : null;

  let viaTcp: HttpResult | null = null;
  let viaSock: HttpResult | null = null;

  if (preferSock) {
    viaSock = await runSock();
    if (viaSock && (viaSock.status === 401 || viaSock.status === 403)) {
      return unauthorizedState(config);
    }
    if (viaSock && is2xx(viaSock.status)) {
      return await connectedState(config, label);
    }
    viaTcp = await runTcp();
  } else {
    viaTcp = await runTcp();
    if (viaTcp && (viaTcp.status === 401 || viaTcp.status === 403)) {
      return unauthorizedState(config);
    }
    if (viaTcp && is2xx(viaTcp.status)) {
      return await connectedState(config, label);
    }
    if (trySock) {
      viaSock = await runSock();
    }
  }

  if (viaSock && (viaSock.status === 401 || viaSock.status === 403)) {
    return unauthorizedState(config);
  }
  if (viaSock && is2xx(viaSock.status)) {
    return await connectedState(config, label);
  }

  // Dev / non-Tauri browser fallback
  const viaBrowser = await browserFetch(config, "GET", "/version", undefined, 2000);
  if (viaBrowser && (viaBrowser.status === 401 || viaBrowser.status === 403)) {
    return unauthorizedState(config);
  }
  if (viaBrowser && is2xx(viaBrowser.status)) {
    return await connectedState(config, label);
  }

  if (viaSock || viaTcp || viaBrowser) {
    const status = (viaSock ?? viaTcp ?? viaBrowser)!.status;
    return {
      status: "unreachable",
      message: clientId
        ? clientUnreachableHint(clientId, config.source)
        : `连接失败（HTTP ${status}）`,
      config,
      currentProxy: null,
      usingMock: false,
      proxiesError: null,
    };
  }

  return {
    status: "unreachable",
    message: clientId
      ? clientUnreachableHint(clientId, config.source)
      : "请先打开并连上你的VPN软件，再点「获取节点」",
    config,
    currentProxy: null,
    usingMock: false,
    proxiesError: null,
  };
}

function unauthorizedState(config: ControllerConfig): ConnectionState {
  return {
    status: "unauthorized",
    message: "连接被拒绝（控制器密钥不匹配），请重启代理软件或刷新连接后重试",
    config,
    currentProxy: null,
    usingMock: false,
    proxiesError: "控制器密钥不匹配，请确认代理软件外部控制器配置后重试",
  };
}

export type GetProxiesResult = {
  nodes: ProxyNode[];
  currentProxy: string | null;
  usingMock: false;
  error: string | null;
  unauthorized: boolean;
};

async function getProxiesViaSlimCommand(
  config: ControllerConfig,
): Promise<GetProxiesResult | null> {
  if (!isTauri()) return null;
  try {
    const res = await invoke<{
      nodes: {
        name: string;
        type: string;
        udp?: boolean;
        xudp?: boolean;
        uot?: boolean;
        tfo?: boolean;
        smux?: boolean;
        mptcp?: boolean;
        alive?: boolean | null;
        lastDelay?: number | null;
        lastDelayAt?: string | null;
      }[];
      /** 策略组摘要表：当前节点判定唯一依赖它 + 上面的 nodes（Rust 不参与判定）。 */
      groups?: SlimGroupLike[];
      status: number;
      error: string | null;
      unauthorized: boolean;
      transport?: string | null;
    }>("mihomo_list_nodes", {
      req: {
        host: config.host,
        port: config.port,
        secret: config.secret,
        timeoutMs: 18000,
        sockPath: config.sockPath ?? null,
      },
    });
    const nodes: ProxyNode[] = (res.nodes ?? []).map((n) => ({
      ...toNode(n.name, {
        name: n.name,
        type: n.type,
        udp: n.udp,
        xudp: n.xudp,
        uot: n.uot,
        tfo: n.tfo,
        smux: n.smux,
        mptcp: n.mptcp,
      }),
      alive: n.alive ?? undefined,
      lastDelay: n.lastDelay ?? undefined,
      lastDelayAt: n.lastDelayAt ?? undefined,
    }));
    return {
      nodes,
      currentProxy: resolveCurrentProxyFromSlim(res.nodes ?? [], res.groups ?? []),
      usingMock: false,
      error: res.error ?? (nodes.length === 0 ? "节点列表为空" : null),
      unauthorized: !!res.unauthorized,
    };
  } catch (e) {
    const detail = e instanceof Error ? e.message : String(e ?? "");
    return {
      nodes: [],
      currentProxy: null,
      usingMock: false,
      error: detail
        ? `mihomo_list_nodes 失败: ${detail}`
        : "mihomo_list_nodes 调用失败",
      unauthorized: false,
    };
  }
}

export async function getProxies(config: ControllerConfig): Promise<GetProxiesResult> {
  const empty = (
    error: string,
    unauthorized = false,
  ): GetProxiesResult => ({
    nodes: [],
    currentProxy: null,
    usingMock: false,
    error,
    unauthorized,
  });

  // Prefer slim Rust command — strips history arrays and caps body size.
  const slim = await getProxiesViaSlimCommand(config);
  if (slim) return slim;

  // Fallback: full /proxies via TCP/unix/browser (dev / non-Tauri).
  const controller = await openControllerRead(config, "/proxies", 18000);
  const res = controller?.response;
  if (!res || !controller) {
    return empty("暂时读不到节点列表，请确认软件已打开并点「获取节点」");
  }
  if (res.status === 401 || res.status === 403) {
    return empty(
      "连接被拒绝（控制器密钥不匹配），请重启代理软件或刷新连接后重试",
      true,
    );
  }
  if (!is2xx(res.status) || !res.json) {
    if (is2xx(res.status) && !res.json) {
      return empty(
        `读节点列表失败（响应异常），请再点「获取节点」`,
      );
    }
    return empty(
      `读节点列表失败，请确认软件已打开并点「获取节点」`,
    );
  }

  const obj = res.json as { proxies?: Record<string, ProxyInfo> };
  let proxies = obj.proxies ?? {};
  if (missingProviderNames(proxies).length > 0) {
    const providers = await controller.read("/providers/proxies", 5000);
    const providerMap = (providers?.json as { providers?: ProviderMap } | null)?.providers;
    if (providers && is2xx(providers.status) && providerMap && typeof providerMap === "object") {
      proxies = mergeProviderNodes(proxies, providerMap);
    }
  }
  let filtered = filterNodes(proxies);
  if (filtered.nodes.length === 0) {
    filtered = resolveNodesFromGroups(proxies);
  }
  if (filtered.nodes.length === 0) {
    return {
      nodes: [],
      currentProxy: filtered.currentProxy,
      usingMock: false,
      error:
        "已连上软件，但还没有可用节点，请确认该软件里订阅已加载",
      unauthorized: false,
    };
  }
  return {
    nodes: filtered.nodes,
    currentProxy: filtered.currentProxy,
    usingMock: false,
    error: missingProviderNames(proxies).length > 0
      ? "已连上软件，但部分订阅节点未能唯一识别"
      : null,
    unauthorized: false,
  };
}

export async function probeDelay(
  config: ControllerConfig,
  nodeName: string,
  url: string,
  timeout = 2500,
): Promise<number | null> {
  const enc = encodeURIComponent(nodeName);
  const path = `/proxies/${enc}/delay?timeout=${timeout}&url=${encodeURIComponent(url)}`;
  const controller = await openControllerRead(config, path, timeout + 800);
  let res = controller?.response;
  if (res?.status === 404 && controller) {
    // Provider-only nodes are not addressable via /proxies/:name in Mihomo.
    // Do not call a whole-provider healthcheck (it probes unrelated nodes).
    const providers = await controller.read("/providers/proxies", 5000);
    const providerMap = (providers?.json as { providers?: ProviderMap } | null)?.providers;
    if (!providers || !is2xx(providers.status) || !providerMap || typeof providerMap !== "object") return null;
    const found = resolveProviderNode(providerMap, nodeName);
    if (!found) return null;
    const providerPath = `/providers/proxies/${encodeURIComponent(found.provider)}/${enc}/healthcheck?timeout=${timeout}&url=${encodeURIComponent(url)}`;
    res = await controller.read(providerPath, timeout + 800) ?? undefined;
  }
  if (!res || res.status !== 200 || !res.json) return null;
  const delay = (res.json as { delay?: number }).delay;
  return typeof delay === "number" && Number.isFinite(delay) && delay > 0 ? delay : null;
}


/**
 * 查找包含该节点（或可通过嵌套子组切换到该节点）的顶层主 Selector 组名。
 * 仅用于用户主动触发「测单个 / 测全部」时的临时切换；其它路径不要调用 switchProxy。
 */
export async function findSelectorGroup(
  config: ControllerConfig,
  nodeName: string,
): Promise<string | null> {
  const snap = await resolveSelectorSnapshot(config, nodeName);
  return snap?.group ?? null;
}

/**
 * 解析包含指定节点（或当前选中）的主控策略组，并读出组内 `now` 及嵌套恢复链 `restoreChain`。
 * 用于「测单个 / 测全部」开始前记录、结束后强制切回。
 */
export async function resolveSelectorSnapshot(
  config: ControllerConfig,
  nodeName?: string | null,
): Promise<SelectorSnapshot | null> {
  const res = await httpApi(config, "GET", "/proxies", undefined, 18000);
  if (!res || !is2xx(res.status) || !res.json) return null;
  const obj = res.json as { proxies?: Record<string, ProxyInfo> };
  const proxies = obj.proxies ?? {};
  return resolveSelectorSnapshotFromMap(proxies, nodeName);
}

export async function switchProxy(
  config: ControllerConfig,
  group: string,
  name: string,
): Promise<boolean> {
  const enc = encodeURIComponent(group);
  const res = await httpApi(config, "PUT", `/proxies/${enc}`, { name });
  if (res && is2xx(res.status)) return true;

  // 若目标节点位于主组下属的地区子组中（如「🚀 节点选择」->「🇺🇸 美国节点」-> 节点），
  // 自动查询 /proxies 计算多步切换路径并依次执行
  const mapRes = await httpApi(config, "GET", "/proxies", undefined, 6000);
  if (!mapRes || !is2xx(mapRes.status) || !mapRes.json) return false;
  const proxies = (mapRes.json as { proxies?: Record<string, ProxyInfo> }).proxies ?? {};
  const steps = resolveSwitchPlanFromMap(proxies, name, group);
  if (steps.length === 0) return false;
  for (const step of steps) {
    const stepEnc = encodeURIComponent(step.group);
    const r = await httpApi(config, "PUT", `/proxies/${stepEnc}`, { name: step.name });
    if (!r || !is2xx(r.status)) return false;
  }
  return true;
}

/** 切回原先节点（含嵌套子组链）；任一步失败返回 false（调用方必须向用户报错，不可静默）。 */
export async function restoreProxy(
  config: ControllerConfig,
  snapshot: SelectorSnapshot,
): Promise<boolean> {
  if (!snapshot.now) return false;
  if (snapshot.restoreChain && snapshot.restoreChain.length > 0) {
    let allOk = true;
    // 先恢复内层子组，最后恢复顶层主组
    for (let i = snapshot.restoreChain.length - 1; i >= 0; i--) {
      const item = snapshot.restoreChain[i]!;
      const enc = encodeURIComponent(item.group);
      const res = await httpApi(config, "PUT", `/proxies/${enc}`, { name: item.now });
      if (!res || !is2xx(res.status)) {
        allOk = false;
      }
    }
    return allOk;
  }
  const enc = encodeURIComponent(snapshot.group);
  const res = await httpApi(config, "PUT", `/proxies/${enc}`, { name: snapshot.now });
  return !!res && is2xx(res.status);
}

export async function closeConnections(config: ControllerConfig): Promise<boolean> {
  const res = await httpApi(config, "DELETE", "/connections");
  return !!res && is2xx(res.status);
}

export async function getVersion(config: ControllerConfig): Promise<string | null> {
  const res = await httpApi(config, "GET", "/version");
  if (!res || res.status !== 200) return null;
  const v = res.json as { version?: string; meta?: boolean };
  return v?.version ?? "ok";
}

/** Expose mock raw for tests / debug */
export { mockProxiesRaw };

/** Best-effort /rules summary for split-routing sample (not a full audit). */
export type RulesSummary = {
  total: number;
  directCount: number;
  rejectCount: number;
  otherCount: number;
  cnHintCount: number;
  samples: string[];
  error: string | null;
};

export async function getRulesSummary(
  config: ControllerConfig,
): Promise<RulesSummary> {
  const empty = (error: string): RulesSummary => ({
    total: 0,
    directCount: 0,
    rejectCount: 0,
    otherCount: 0,
    cnHintCount: 0,
    samples: [],
    error,
  });

  const res = await httpApi(config, "GET", "/rules", undefined, 8000);
  if (!res) return empty("无法读取 /rules（超时或未连接）");
  if (res.status === 401 || res.status === 403) {
    return empty("读取 /rules 未授权（401/403）");
  }
  if (!is2xx(res.status) || !res.json) {
    return empty(`读取 /rules 失败（HTTP ${res.status}）`);
  }

  const obj = res.json as {
    rules?: { type?: string; payload?: string; proxy?: string }[];
  };
  const rules = obj.rules ?? [];
  let directCount = 0;
  let rejectCount = 0;
  let otherCount = 0;
  let cnHintCount = 0;
  const samples: string[] = [];

  for (const r of rules) {
    const proxy = (r.proxy ?? "").toUpperCase();
    const typ = r.type ?? "?";
    const payload = r.payload ?? "";
    if (proxy === "DIRECT") directCount += 1;
    else if (proxy === "REJECT" || proxy === "REJECT-DROP") rejectCount += 1;
    else otherCount += 1;

    const blob = `${typ} ${payload} ${proxy}`.toUpperCase();
    if (
      blob.includes("CN") ||
      blob.includes("CHINA") ||
      payload.includes("baidu") ||
      payload.includes("qq.com") ||
      payload.includes("geolocation")
    ) {
      cnHintCount += 1;
      if (samples.length < 6) {
        samples.push(`${typ}(${payload || "-"}) → ${r.proxy ?? "?"}`);
      }
    } else if (samples.length < 3 && proxy === "DIRECT") {
      samples.push(`${typ}(${payload || "-"}) → DIRECT`);
    }
  }

  return {
    total: rules.length,
    directCount,
    rejectCount,
    otherCount,
    cnHintCount,
    samples,
    error: null,
  };
}
