import {
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { CheckCardView } from "../components/CheckCardView";
import { ThemeToggle } from "../components/ThemeToggle";
import { NodeCard } from "../components/NodeCard";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type { CheckCard, EgressReport } from "../lib/egress";
import {
  CLIENT_OPTIONS,
  clientLabel,
  normalizeClientId,
  type ClientId,
  type ConnectionState,
  type ControllerConfig,
  type ProxyNode,
} from "../lib/mihomo";
import {
  starRank,
  runLightGate,
  type GateResult,
  type NodeScoreResult,
} from "../lib/score";
import {
  ENV_PLACEHOLDERS,
  NODE_PLACEHOLDERS,
  runEnv,
  testAll as runTestAll,
  testOne,
  type RunnerHooks,
  type RunnerProgress,
} from "../lib/runner";

type TestMode = "current" | "all";

/** 瀑布流列数：单卡固定 298px + 9px 列距，1–8 列封顶。
 * 余量 ≈ main 左右 padding + tray 左右 pad。
 * 1920 宽约 6 列；(vw-72)/307。 */
function colCountFor(vw: number): number {
  return Math.min(8, Math.max(1, Math.floor((vw - 72) / 307)));
}

/** 进度文案拆成「阶段」+「节点名」。全部检测时节点名在 CSS 里定宽截断（title 给全名）。 */
function splitProgressDisplay(p: RunnerProgress | null): {
  phase: string;
  node?: string;
} | null {
  if (!p?.text) return null;
  const embedded = /^(.*?)（(.+?)）$/.exec(p.text);
  if (embedded) {
    return { phase: embedded[1]!.trimEnd(), node: embedded[2] };
  }
  if (p.testingNode && !p.text.includes(p.testingNode)) {
    return { phase: p.text, node: p.testingNode };
  }
  return { phase: p.text };
}

export function HomePage({
  connection,
  busy,
  nodes,
  clientId,
  onClientIdChange,
  onRefresh,
  manual,
  forceMock,
  onChangeManual,
  onResetManual,
  onForceMockChange,
  onRefreshAdvanced,
}: {
  connection: ConnectionState;
  busy?: boolean;
  nodes: ProxyNode[];
  clientId: ClientId | null;
  onClientIdChange: (id: ClientId) => void;
  onRefresh?: () => Promise<unknown> | void;
  manual: Partial<ControllerConfig>;
  forceMock: boolean;
  onChangeManual: (patch: Partial<ControllerConfig>) => void;
  onResetManual: () => void;
  onForceMockChange: (v: boolean) => void;
  onRefreshAdvanced: (override?: Partial<ControllerConfig>) => Promise<unknown>;
}) {
  const [nodeCards, setNodeCards] = useState<CheckCard[]>(NODE_PLACEHOLDERS);
  const [envCards, setEnvCards] = useState<CheckCard[]>(ENV_PLACEHOLDERS);
  const [report, setReport] = useState<EgressReport | null>(null);
  const [running, setRunning] = useState(false);
  const [progress, setProgress] = useState<RunnerProgress | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  const [mode, setMode] = useState<TestMode>("current");
  const [gate, setGate] = useState<GateResult | null>(null);
  const [nodeScores, setNodeScores] = useState<NodeScoreResult[]>([]);
  const [envOpen, setEnvOpen] = useState(false);
  const [envRunning, setEnvRunning] = useState(false);
  /** 环境检查完成后在按钮旁显示 ✓成功；下次检查开始时清除 */
  const [envCheckOk, setEnvCheckOk] = useState(false);
  /** 测全部 / 单节点完成后在「测全部节点」旁显示 ✓成功；下次开测时清除 */
  const [nodeTestOk, setNodeTestOk] = useState(false);
  const [allConfirmOpen, setAllConfirmOpen] = useState(false);
  const [switchHint, setSwitchHint] = useState<string | null>(null);
  const [restoreError, setRestoreError] = useState<string | null>(null);
  const abortAllRef = useRef(false);
  const [advancedOpen, setAdvancedOpen] = useState(false);
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [testingNode, setTestingNode] = useState<string | null>(null);
  const [colCount, setColCount] = useState(() =>
    typeof window === "undefined" ? 2 : colCountFor(window.innerWidth),
  );

  const cfg = connection.config;
  const [host, setHost] = useState(manual.host ?? cfg?.host ?? "127.0.0.1");
  const [port, setPort] = useState(String(manual.port ?? cfg?.port ?? 9097));
  const [secret, setSecret] = useState(manual.secret ?? cfg?.secret ?? "");
  const [mixedPort, setMixedPort] = useState(
    String(manual.mixedPort ?? cfg?.mixedPort ?? 7897),
  );

  const clientUnset = !clientId;

  const scoreByName = useMemo(() => {
    const m = new Map<string, (typeof nodeScores)[number]>();
    for (const s of nodeScores) m.set(s.nodeName, s);
    return m;
  }, [nodeScores]);

  const orderedNodes = useMemo(() => {
    const current = connection.currentProxy;
    const ordered = [...nodes];
    if (nodeScores.length > 0) {
      // 三桶排序：已测活的在前（按星级/总分），没测的居中（保持订阅顺序，
      // 当前节点优先），不可用的垫底——测全部过程中死节点边测边出分，
      // 不能让它们反而浮到最前面干扰阅读。
      const bucket = (s?: NodeScoreResult) =>
        !s ? 1 : s.stars === "unavailable" ? 2 : 0;
      ordered.sort((a, b) => {
        const sa = scoreByName.get(a.name);
        const sb = scoreByName.get(b.name);
        const ka = bucket(sa);
        const kb = bucket(sb);
        if (ka !== kb) return ka - kb;
        if (sa && sb) {
          const byStar = starRank(sb.stars) - starRank(sa.stars);
          if (byStar !== 0) return byStar;
          return sb.totalScore - sa.totalScore;
        }
        if (a.name === current) return -1;
        if (b.name === current) return 1;
        return 0;
      });
      return ordered;
    }
    if (current) {
      const i = ordered.findIndex((n) => n.name === current);
      if (i > 0) {
        const [row] = ordered.splice(i, 1);
        ordered.unshift(row);
      }
    }
    return ordered;
  }, [nodes, connection.currentProxy, nodeScores, scoreByName]);

  const mixedPortNum = connection.config?.mixedPort ?? null;

  const upsertNodeCard = (card: CheckCard) => {
    setNodeCards((prev) => {
      const idx = prev.findIndex((p) => p.id === card.id);
      if (idx === -1) return [...prev, card];
      const next = [...prev];
      next[idx] = card;
      return next;
    });
  };

  const upsertEnvCard = (card: CheckCard) => {
    setEnvCards((prev) => {
      const idx = prev.findIndex((p) => p.id === card.id);
      if (idx === -1) return [...prev, card];
      const next = [...prev];
      next[idx] = card;
      return next;
    });
  };

  const onSelectClient = (raw: string) => {
    const picked = CLIENT_OPTIONS.find((o) => o.id === raw);
    if (!picked || picked.disabled) return;
    const id = normalizeClientId(raw);
    if (!id) return;
    onClientIdChange(id);
  };

  const applyAdvanced = async () => {
    const patch: Partial<ControllerConfig> = {
      host,
      port: Number(port) || 9097,
      secret,
      mixedPort: Number(mixedPort) || 7897,
      source: "manual",
    };
    onChangeManual(patch);
    await onRefreshAdvanced(patch);
  };

  const autoDetect = async () => {
    onResetManual();
    setHost("127.0.0.1");
    setPort("9097");
    setSecret("");
    setMixedPort("7897");
    await onRefreshAdvanced({ source: "auto" });
  };

  const refreshAndGate = async () => {
    setRefreshing(true);
    try {
      const next = await onRefresh?.();
      const conn =
        next && typeof next === "object" && "status" in (next as object)
          ? (next as ConnectionState)
          : connection;
      const g = await runLightGate(conn);
      setGate(g);
    } finally {
      setRefreshing(false);
    }
  };

  const ensureGate = async (): Promise<GateResult> => {
    setProgress({ text: "测试条件检查中…" });
    const g = await runLightGate(connection);
    setGate(g);
    // 不置 null：避免测全部接上预检前闪空白/「连…」；由 runner 覆盖或结束时清空
    return g;
  };

  const buildHooks = (): RunnerHooks => ({
    onProgress: setProgress,
    onNodeCards: setNodeCards,
    onUpsertNodeCard: upsertNodeCard,
    onReport: setReport,
    onUpsertScore: (score) => {
      setNodeScores((prev) => {
        const next = prev.filter((x) => x.nodeName !== score.nodeName);
        next.push(score);
        return [...next].sort((a, b) => {
          const byStar = starRank(b.stars) - starRank(a.stars);
          if (byStar !== 0) return byStar;
          return b.totalScore - a.totalScore;
        });
      });
    },
    onScores: setNodeScores,
    onHint: setSwitchHint,
    onRestoreError: setRestoreError,
    onGate: setGate,
    onEnvCards: setEnvCards,
    onUpsertEnvCard: upsertEnvCard,
  });

  const testAll = async () => {
    abortAllRef.current = false;
    setNodeTestOk(false);
    setRunning(true);
    try {
      const ok = await runTestAll(
        {
          connection,
          nodes,
          forceMock,
          mixedPort: mixedPortNum,
          ensureGate,
          shouldAbort: () => abortAllRef.current,
        },
        buildHooks(),
      );
      if (ok) setNodeTestOk(true);
    } finally {
      setRunning(false);
      abortAllRef.current = false;
    }
  };

  const runEnvCheck = async () => {
    setEnvCheckOk(false);
    setEnvRunning(true);
    setEnvOpen(true);
    try {
      const ok = await runEnv(
        {
          connection,
          mixedPort: mixedPortNum,
          exitIp: report?.exitIp ?? null,
        },
        buildHooks(),
      );
      if (ok) setEnvCheckOk(true);
    } finally {
      setEnvRunning(false);
    }
  };

  const onConfirmAll = () => {
    setAllConfirmOpen(false);
    void testAll();
  };

  const onAbortAll = () => {
    abortAllRef.current = true;
    setProgress({ text: "正在停止…", testingNode: undefined });
  };

  // v0.1.10：版本号进原生标题栏（玻璃窗口下正文不再放介绍卡）。
  useEffect(() => {
    if (typeof window !== "undefined" && "__TAURI_INTERNALS__" in window) {
      void getCurrentWindow()
        .setTitle(`Clash伴侣 ${__APP_VERSION__}`)
        .catch(() => {});
    }
  }, []);

  // 步骤③完成判定：三种检测路径（环境检查/测全部/测单节点）任一产出过结果
  const envDone =
    envCards.length > 0 &&
    envCards.every(
      (c) => c.level !== "unknown" && c.level !== "running",
    );
  const detectionDone = nodeScores.length > 0 || envDone;

  const toggleExpand = (name: string) =>
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(name)) next.delete(name);
      else next.add(name);
      return next;
    });

  // 瀑布流列数：Tauri 的 webview 宽度 = 窗口宽度，直接用 window.innerWidth +
  // resize 事件，比 ResizeObserver 可靠 —— 不受"工作区条件渲染、挂载时序"影响。
  useEffect(() => {
    const onResize = () => setColCount(colCountFor(window.innerWidth));
    onResize();
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, []);

  const nodeColumns: ProxyNode[][] = useMemo(
    () => {
      const cols: ProxyNode[][] = Array.from({ length: colCount }, () => []);
      orderedNodes.forEach((n, i) => cols[i % colCount].push(n));
      return cols;
    },
    [orderedNodes, colCount],
  );

  // 测全部的行内进度（0–100；无比例时 null）
  const progressPct =
    progress &&
    typeof progress.current === "number" &&
    typeof progress.total === "number" &&
    progress.total > 0
      ? Math.max(
          0,
          Math.min(100, Math.round((progress.current / progress.total) * 100)),
        )
      : null;
  const progressDisplay = splitProgressDisplay(progress);
  // 进度条始终占位：切换/准备等无比例阶段沿用上一次的比例（运行开始为 0），只换文字，停止按钮不移动
  const lastPctRef = useRef(0);
  if (!running) lastPctRef.current = 0;
  else if (progressPct != null) lastPctRef.current = progressPct;
  const barPct = progressPct ?? lastPctRef.current;

  const allExpanded =
    orderedNodes.length > 0 && orderedNodes.every((n) => expanded.has(n.name));

  const toggleExpandAll = () =>
    setExpanded(
      allExpanded ? new Set() : new Set(orderedNodes.map((n) => n.name)),
    );

  // 单独测某个节点：完整深测（runNodeDiagnostics）；非当前节点临时切换后切回。
  // NodeCard「再测」由 Ellie 接 onTest；此处保持接口稳定。
  const testOneNode = async (node: ProxyNode) => {
    if (running) return;
    setNodeTestOk(false);
    setRestoreError(null);
    setRunning(true);
    setTestingNode(node.name);
    try {
      const ok = await testOne(
        {
          node,
          connection,
          forceMock,
          mixedPort: mixedPortNum,
          ensureGate,
        },
        buildHooks(),
      );
      if (ok) {
        setExpanded((prev) => new Set(prev).add(node.name));
        setNodeTestOk(true);
      }
    } finally {
      setRunning(false);
      setTestingNode(null);
      setProgress(null);
    }
  };

  return (
    <div className="home-page">
      <div className="flow-row">
        <div className="flow-hint" aria-label="使用步骤">
          <span className={`fh-step ${gate?.ok ? "done" : "cur"}`}>
            <i>{gate?.ok ? "✓" : "1"}</i>打开你的VPN软件并连上一个可用节点
            <span className="fh-arrow" aria-hidden="true">→</span>
          </span>
          <span className="fh-rest">
            <span className={`fh-step ${gate?.ok ? "done" : "todo"}`}>
              <i>{gate?.ok ? "✓" : "2"}</i>在下方选择你使用的VPN软件 · 获取节点
              <span className="fh-arrow" aria-hidden="true">→</span>
            </span>
            <span
              className={`fh-step ${!gate?.ok ? "todo" : detectionDone ? "done" : "cur"}`}
            >
              <i>{!gate?.ok ? "3" : detectionDone ? "✓" : "3"}</i>进行检测
            </span>
          </span>
        </div>
        <ThemeToggle />
      </div>

      <div className="app-header">
        <div className="home-ops-controls">
          <div className="picker-group">
            <label className="client-picker-label home-block-title" htmlFor="home-client-select">
              你在用哪款软件？
            </label>
            <select
              id="home-client-select"
              className="client-picker-select"
              value={clientId ?? ""}
              onChange={(e) => onSelectClient(e.target.value)}
              title={
                clientUnset
                  ? "先选软件，再点「获取节点」"
                  : CLIENT_OPTIONS.find((o) => o.id === clientId)?.hint
              }
            >
              <option value="" disabled>
                请选择…
              </option>
              {CLIENT_OPTIONS.map((o) => (
                <option key={o.id} value={o.id} disabled={o.disabled}>
                  {o.label}
                </option>
              ))}
            </select>
          </div>
          <div className="action-with-ok">
            <button
              className="btn btn-primary btn-sm home-ops-refresh action-btn"
              type="button"
              disabled={!!busy || refreshing || running || clientUnset}
              title={clientUnset ? "请先选择软件" : undefined}
              onClick={() => void refreshAndGate()}
            >
              {busy || refreshing ? "获取中…" : "获取节点"}
            </button>
            {!refreshing &&
            (connection.status === "connected" && gate?.ok ? (
              <span className="fetch-ok" title={connection.message}>
                <span className="status-ok-mark">✓</span>成功
              </span>
            ) : gate && !gate.ok ? (
              <>
                <span className="fetch-fail">
                  <span className="status-fail-mark">✗</span>失败
                </span>
                <span className="fetch-fail-msg" role="status">
                  {gate.message}
                </span>
              </>
            ) : null)}
          </div>
        </div>
      </div>

      {gate?.ok && orderedNodes.length > 0 ? (
        <>
          <div className="env-section home-tray">
            <div className="env-head">
              <div className="t">
                <span className="home-block-title">环境泄漏检查</span>
                <span className="s">对当前出口体检 · 不需先测节点</span>
              </div>
              <div className="env-head-actions">
                <button
                  type="button"
                  className="btn btn-primary btn-sm action-btn"
                  disabled={envRunning || running}
                  onClick={() => void runEnvCheck()}
                >
                  {envRunning ? "检查中…" : envOpen ? "重新检查环境" : "开始环境检查"}
                </button>
                {!envRunning && envCheckOk ? (
                  <span className="fetch-ok" role="status">
                    <span className="status-ok-mark">✓</span>成功
                  </span>
                ) : null}
              </div>
            </div>
            {envOpen ? (
              <>
                {mixedPortNum == null || mixedPortNum <= 0 ? (
                  <div className="note note-compact env-tray-note">
                    当前未检测到代理，境外探测点不可达，结论不代表 VPN 表现。
                  </div>
                ) : null}
                <div className="card-grid card-grid-home env-tray-grid">
                  {envCards.map((c) => (
                    <CheckCardView key={c.id} card={c} />
                  ))}
                </div>
              </>
            ) : null}
          </div>

          <div className="node-section home-tray">
            <div className="ws-ops">
              <span className="t home-block-title">节点检测</span>
              <div className="ws-ops-right">
                <div className="action-with-ok">
                  <button
                    type="button"
                    className="btn btn-primary btn-sm action-btn"
                    disabled={running || clientUnset}
                    onClick={() => {
                      setMode("all");
                      setRestoreError(null);
                      setAllConfirmOpen(true);
                    }}
                  >
                    {running && mode === "all" ? "测全部中…" : "测全部节点"}
                  </button>
                  {!running && nodeTestOk ? (
                    <span className="fetch-ok" role="status">
                      <span className="status-ok-mark">✓</span>成功
                    </span>
                  ) : null}
                  {restoreError ? (
                    <span className="ws-restore-error" role="alert">
                      {restoreError}
                    </span>
                  ) : null}
                </div>
                {running && mode === "all" ? (
                  <div
                    className="ws-progress-group"
                    role="status"
                    aria-live="polite"
                  >
                    <span className="ws-progress-main">
                      <span className="ws-progress-text">
                        {progressDisplay ? (
                          <>
                            <span className="ws-progress-phase">
                              {progressDisplay.phase}
                            </span>
                            {progressDisplay.node ? (
                              <span className="ws-progress-node">
                                （
                                <span
                                  className="ws-progress-node-name"
                                  title={progressDisplay.node}
                                >
                                  {progressDisplay.node}
                                </span>
                                ）
                              </span>
                            ) : null}
                          </>
                        ) : null}
                      </span>
                      <span
                        className="ws-progress-bar"
                        role="progressbar"
                        aria-valuemin={0}
                        aria-valuemax={100}
                        aria-valuenow={barPct}
                      >
                        <span
                          className="ws-progress-fill"
                          style={{ width: `${barPct}%` }}
                        />
                      </span>
                    </span>
                    <button
                      type="button"
                      className="btn btn-sm ws-abort-btn"
                      onClick={onAbortAll}
                    >
                      停止并切回
                    </button>
                  </div>
                ) : running && testingNode ? (
                  <div className="ws-progress" role="status" aria-live="polite">
                    <span className="ws-progress-text">
                      {progressDisplay ? (
                        <>
                          <span className="ws-progress-phase">
                            {progressDisplay.phase}
                          </span>
                          {progressDisplay.node ? (
                            <span className="ws-progress-node">
                              （{progressDisplay.node}）
                            </span>
                          ) : null}
                        </>
                      ) : (
                        <span className="ws-progress-phase">
                          正在检测 {testingNode}…
                        </span>
                      )}
                    </span>
                  </div>
                ) : null}
                {switchHint && !restoreError ? (
                  <span className="ws-error" role="alert">
                    {switchHint}
                  </span>
                ) : null}
                <button
                  type="button"
                  className="ws-expand-link"
                  onClick={toggleExpandAll}
                  title={allExpanded ? "收起全部节点详情" : "展开全部节点详情"}
                >
                  {allExpanded ? "收起全部详情" : "展开全部详情"}
                </button>
              </div>
            </div>

            <div className="node-flow">
              {nodeColumns.map((col, ci) => (
                <div className="flow-col" key={ci}>
                  {col.map((n) => (
                    <NodeCard
                      key={n.name}
                      node={n}
                      score={scoreByName.get(n.name)}
                      liveCards={testingNode === n.name ? nodeCards : undefined}
                      isCurrent={n.name === connection.currentProxy}
                      expanded={expanded.has(n.name)}
                      testing={testingNode === n.name}
                      retestLocked={running}
                      onToggle={() => toggleExpand(n.name)}
                      onTest={() => void testOneNode(n)}
                    />
                  ))}
                </div>
              ))}
            </div>
          </div>
        </>
      ) : null}

      {allConfirmOpen && mode === "all" ? (
        <div className="confirm-overlay">
          <div
            className="confirm-panel card"
            role="dialog"
            aria-modal="true"
            aria-label="确认测全部"
          >
            <p className="confirm-body">
              点击「确定」后，将逐个检测所有节点（约几分钟），期间会切换出口节点并消耗流量，建议暂时不要进行支付、登录等重要操作。测完会自动切回原节点。
            </p>
            <div className="toolbar" style={{ marginBottom: 0 }}>
              <button
                className="btn btn-primary"
                type="button"
                disabled={clientUnset}
                onClick={onConfirmAll}
              >
                确定
              </button>
              <button
                className="btn"
                type="button"
                onClick={() => setAllConfirmOpen(false)}
              >
                取消
              </button>
            </div>
          </div>
        </div>
      ) : null}


      <div className="fold-panel card">
        <button
          type="button"
          className="fold-toggle"
          aria-expanded={advancedOpen}
          onClick={() => setAdvancedOpen((v) => !v)}
        >
          {advancedOpen ? "收起高级" : "高级（一般不用）"}
        </button>
        {advancedOpen ? (
          <div className="fold-body">
            <p className="muted fold-hint">
              当前软件：<strong>{clientLabel(clientId)}</strong>
              {clientId
                ? " — 连接参数会按该软件自动发现，一般不用改。"
                : " — 请先选择你正在用的软件。"}
            </p>
            <div className="form-grid" style={{ marginBottom: 12 }}>
              <label style={{ display: "flex", alignItems: "center", gap: 8 }}>
                <input
                  type="checkbox"
                  checked={forceMock}
                  onChange={(e) => onForceMockChange(e.target.checked)}
                />
                <span>
                  <strong>Mock 演示模式</strong>
                  <span className="muted"> — 用假数据预览界面，不连真实软件</span>
                </span>
              </label>
            </div>
            <div className="form-grid">
              <p className="muted fold-hint">
                以下字段仅在自动连接失败、或你清楚自己改过控制口时使用。
              </p>
              <div className="field">
                <label>Host</label>
                <input
                  value={host}
                  onChange={(e) => setHost(e.target.value)}
                  disabled={forceMock}
                />
              </div>
              <div className="field">
                <label>Port（external-controller）</label>
                <input
                  value={port}
                  onChange={(e) => setPort(e.target.value)}
                  disabled={forceMock}
                />
              </div>
              <div className="field">
                <label>Secret（不会写入日志 / 仓库）</label>
                <input
                  type="password"
                  value={secret}
                  onChange={(e) => setSecret(e.target.value)}
                  autoComplete="off"
                  disabled={forceMock}
                />
              </div>
              <div className="field">
                <label>mixed-port（经代理探针）</label>
                <input
                  value={mixedPort}
                  onChange={(e) => setMixedPort(e.target.value)}
                  disabled={forceMock}
                />
              </div>
              <div className="toolbar" style={{ marginBottom: 0 }}>
                <button
                  className="btn btn-primary"
                  type="button"
                  disabled={!!busy || forceMock}
                  onClick={() => void applyAdvanced()}
                >
                  保存并测试连接
                </button>
                <button
                  className="btn"
                  type="button"
                  disabled={!!busy || forceMock}
                  onClick={() => void autoDetect()}
                >
                  重新自动发现
                </button>
              </div>
              <p className="muted fold-hint">
                来源：{cfg?.source ?? "—"}
                <br />
                Unix 套接字：{cfg?.sockPath ?? "（无）"}
                <br />
                mixed-port：{cfg?.mixedPort ?? "—"}
                <br />
                当前节点：{connection.currentProxy ?? "—"}
              </p>
            </div>
          </div>
        ) : null}
      </div>

      <details className="about-footer">
        <summary>关于 Clash伴侣</summary>
        <div className="a-body">
          仓库：
          <a
            href="https://github.com/JTee77/egress-checker"
            target="_blank"
            rel="noreferrer"
          >
            github.com/JTee77/egress-checker
          </a>
          <br />
          致谢：Clash Verge / mihomo 社区。
          <br />
          请我喝杯咖啡 ☕（占位：链接或二维码待定）
        </div>
      </details>
    </div>
  );
}
