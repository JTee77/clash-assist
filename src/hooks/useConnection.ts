import { useCallback, useRef, useState } from "react";
import {
  CLIENT_OPTIONS,
  CLIENT_STORAGE_KEY,
  baseSource,
  defaultConfig,
  discoverAndProbe,
  getProxies,
  mockNodes,
  normalizeClientId,
  type ClientId,
  type ConnectionState,
  type ControllerConfig,
  type ProxyNode,
} from "../lib/mihomo";

const MOCK_PREF_KEY = "clash-assist.forceMock";

/** 0.2.0 改名前的 localStorage 键（egress-checker.*）：读新键为空时回退读一次并迁移到新键。 */
function readPref(key: string): string | null {
  const v = localStorage.getItem(key);
  if (v !== null) return v;
  const legacyKey = key.replace(/^clash-assist\./, "egress-checker.");
  const old = localStorage.getItem(legacyKey);
  if (old !== null) {
    try {
      localStorage.setItem(key, old);
      localStorage.removeItem(legacyKey);
    } catch {
      /* ignore */
    }
  }
  return old;
}

function readForceMock(): boolean {
  try {
    return readPref(MOCK_PREF_KEY) === "1";
  } catch {
    return false;
  }
}

function sniffCandidateOrder(preferred: ClientId | null): ClientId[] {
  const selectable = CLIENT_OPTIONS.filter((o) => !o.disabled).map((o) => o.id);
  if (preferred && selectable.includes(preferred)) {
    return [preferred, ...selectable.filter((id) => id !== preferred)];
  }
  return selectable;
}

/**
 * 判断某客户端是否在本地硬盘上留下了真实配置/Socket 痕迹（而非纯默认回退）。
 * 只看去掉 `;key=value` 后缀后的标签（PR52 Ellie must-fix 2）：`verge-pipe:b;pipe-insecure-dacl=a`
 * 是「b 可信且已选中、a 被拒」，算已安装；被拒管道的警告由 DiagnosticsNotices 按全部候选来源单独显示。
 * `defaults` / `unset-default` / `*:defaults` / `preset-*`（含带后缀的 `defaults;…`）不算。
 */
export function hasClientFootprint(state: ConnectionState): boolean {
  const base = baseSource(state.config?.source ?? "");
  if (!base || base === "defaults" || base === "unset-default") return false;
  if (base.endsWith(":defaults") || base.startsWith("preset-")) return false;
  return true;
}

const UNDETECTED_APP_MESSAGE =
  "未检测到正在运行的VPN软件。请先打开软件并连上节点后点「刷新节点」。";

export function useConnection() {
  const [forceMock, setForceMockState] = useState<boolean>(readForceMock);
  const [clientId, setClientIdState] = useState<ClientId | null>(() => {
    try {
      const v = readPref(CLIENT_STORAGE_KEY);
      const id = normalizeClientId(v);
      if (!id && v) {
        try {
          localStorage.removeItem(CLIENT_STORAGE_KEY);
        } catch {
          /* ignore */
        }
      }
      return id;
    } catch {
      return null;
    }
  });
  const [state, setState] = useState<ConnectionState>({
    status: "unknown",
    message: "尚未检测连接",
    config: defaultConfig(),
    currentProxy: null,
    usingMock: false,
    proxiesError: null,
  });
  const [nodes, setNodes] = useState<ProxyNode[]>([]);
  /** 最近一次刷新里所有候选的发现来源（诊断提示用：InsecureDacl 等不能只看最终选中的那个）。 */
  const [discoverySources, setDiscoverySources] = useState<string[]>([]);
  const [manual, setManual] = useState<Partial<ControllerConfig>>({});
  const [busy, setBusy] = useState(false);
  const refreshGen = useRef(0);

  const setForceMock = (v: boolean) => {
    setForceMockState(v);
    try {
      localStorage.setItem(MOCK_PREF_KEY, v ? "1" : "0");
    } catch {
      /* ignore */
    }
  };

  const setClientId = useCallback((id: ClientId | null) => {
    setClientIdState(id);
    try {
      if (id) localStorage.setItem(CLIENT_STORAGE_KEY, id);
      else localStorage.removeItem(CLIENT_STORAGE_KEY);
    } catch {
      /* ignore */
    }
  }, []);

  const refresh = useCallback(
    async (
      override?: Partial<ControllerConfig>,
      mockOverride?: boolean,
      explicitClientId?: ClientId | null,
    ) => {
      const gen = ++refreshGen.current;
      setBusy(true);
      try {
        const useMock = mockOverride ?? forceMock;
        if (useMock) {
          const cfg = { ...defaultConfig(), ...manual, ...override };
          const list = mockNodes();
          const next: ConnectionState = {
            status: "mock",
            message: "已启用 Mock 演示模式（未连接真实软件）",
            config: cfg,
            currentProxy: list[0]?.name ?? null,
            usingMock: true,
            proxiesError: null,
          };
          if (gen !== refreshGen.current) return next;
          setState(next);
          setNodes(list);
          setDiscoverySources([]);
          return next;
        }

        const merged = { ...manual, ...override };
        const manualArg = Object.keys(merged).length ? merged : undefined;

        // 1) 若用户在下拉框显式选择了某款软件，则仅针对该软件探测并报该软件的专属提示
        if (explicitClientId) {
          setClientId(explicitClientId);
          const next = await discoverAndProbe(manualArg, explicitClientId);
          if (gen !== refreshGen.current) return next;
          setDiscoverySources(next.config?.source ? [next.config.source] : []);

          if (
            !next.config ||
            next.status === "unreachable" ||
            next.status === "unauthorized"
          ) {
            setState({ ...next, proxiesError: next.proxiesError ?? null });
            setNodes([]);
            return next;
          }

          const list = await getProxies(next.config);
          if (gen !== refreshGen.current) return next;
          setNodes(list.nodes);

          if (list.unauthorized) {
            const unauthorized: ConnectionState = {
              ...next,
              status: "unauthorized",
              message:
                "连接被拒绝（控制器密钥不匹配），请重启代理软件或刷新连接后重试",
              usingMock: false,
              currentProxy: null,
              proxiesError: list.error,
            };
            setState(unauthorized);
            return unauthorized;
          }

          const mergedState: ConnectionState = {
            ...next,
            usingMock: false,
            currentProxy: list.currentProxy ?? next.currentProxy,
            proxiesError: list.error,
            message: list.error
              ? `${next.message} — ${list.error}`
              : next.message,
          };
          setState(mergedState);
          return mergedState;
        }

        // 2) 自动嗅探：依次探测候选软件
        const candidates = sniffCandidateOrder(clientId);
        const results: Array<{ id: ClientId; state: ConnectionState }> = [];
        let connectedHit: { id: ClientId; state: ConnectionState } | null = null;

        for (const cand of candidates) {
          const probed = await discoverAndProbe(manualArg, cand);
          if (gen !== refreshGen.current) return probed;
          results.push({ id: cand, state: probed });
          if (probed.status === "connected") {
            connectedHit = { id: cand, state: probed };
            break;
          }
        }
        setDiscoverySources(
          results.flatMap((r) => (r.state.config?.source ? [r.state.config.source] : [])),
        );

        // 2a) 有软件在线响应：直接选中并拉取节点
        if (connectedHit && connectedHit.state.config) {
          setClientId(connectedHit.id);
          const next = connectedHit.state;
          const list = await getProxies(next.config!);
          if (gen !== refreshGen.current) return next;
          setNodes(list.nodes);

          if (list.unauthorized) {
            const unauthorized: ConnectionState = {
              ...next,
              status: "unauthorized",
              message:
                "连接被拒绝（控制器密钥不匹配），请重启代理软件或刷新连接后重试",
              usingMock: false,
              currentProxy: null,
              proxiesError: list.error,
            };
            setState(unauthorized);
            return unauthorized;
          }

          const mergedState: ConnectionState = {
            ...next,
            usingMock: false,
            currentProxy: list.currentProxy ?? next.currentProxy,
            proxiesError: list.error,
            message: list.error
              ? `${next.message} — ${list.error}`
              : next.message,
          };
          setState(mergedState);
          return mergedState;
        }

        // 2b) 均未连上：检查是否只有唯一一款软件在本地有配置痕迹（或者只有唯一一款报 401）
        const authHit = results.find((r) => r.state.status === "unauthorized");
        if (authHit) {
          setClientId(authHit.id);
          setState({
            ...authHit.state,
            proxiesError: authHit.state.proxiesError ?? null,
          });
          setNodes([]);
          return authHit.state;
        }

        const withFootprint = results.filter((r) => hasClientFootprint(r.state));
        if (withFootprint.length === 1) {
          const single = withFootprint[0]!;
          setClientId(single.id);
          setState({
            ...single.state,
            proxiesError: single.state.proxiesError ?? null,
          });
          setNodes([]);
          return single.state;
        }

        // 2c) 两款都装了且都没开（或两款都没装）：若之前已有明确的 clientId 选择则保留，
        // 若首次打开无明确选择则不替用户瞎猜，保持未选状态并提示用户打开软件或手动选择
        if (clientId) {
          const preferredRes =
            results.find((r) => r.id === clientId)?.state ?? results[0]?.state;
          if (preferredRes) {
            setState({
              ...preferredRes,
              proxiesError: preferredRes.proxiesError ?? null,
            });
            setNodes([]);
            return preferredRes;
          }
        }

        const undecided: ConnectionState = {
          status: "unreachable",
          message: UNDETECTED_APP_MESSAGE,
          config: { ...defaultConfig(), ...manual, ...override },
          currentProxy: null,
          usingMock: false,
          proxiesError: UNDETECTED_APP_MESSAGE,
        };
        setState(undecided);
        setNodes([]);
        return undecided;
      } catch (err) {
        const message =
          err instanceof Error ? err.message : String(err ?? "连接检测失败");
        const next: ConnectionState = {
          status: "unreachable",
          message,
          config: { ...defaultConfig(), ...manual, ...override },
          currentProxy: null,
          usingMock: false,
          proxiesError: message,
        };
        if (gen === refreshGen.current) {
          setState(next);
          setNodes([]);
          setDiscoverySources([]);
        }
        return next;
      } finally {
        if (gen === refreshGen.current) {
          setBusy(false);
        }
      }
    },
    [manual, forceMock, clientId, setClientId],
  );

  const updateManual = (patch: Partial<ControllerConfig>) => {
    setManual((m) => ({ ...m, ...patch }));
  };

  const resetManual = () => {
    setManual({});
  };

  return {
    state,
    nodes,
    discoverySources,
    busy,
    manual,
    forceMock,
    setForceMock,
    clientId,
    setClientId,
    updateManual,
    resetManual,
    refresh,
  };
}
