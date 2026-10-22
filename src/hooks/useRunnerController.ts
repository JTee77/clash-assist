import { useCallback, useEffect, useRef, useState } from "react";
import type { CheckCard, EgressReport } from "../lib/egress";
import type {
  ClientId,
  ConnectionState,
  ProxyNode,
} from "../lib/mihomo";
import {
  runLightGate,
  type GateResult,
  type NodeScoreResult,
} from "../lib/score";
import {
  ENV_PLACEHOLDERS,
  NODE_PLACEHOLDERS,
  cacheScopeFor,
  formatCachedTime,
  loadCachedResults,
  mergeScoreList,
  runEnv,
  saveCachedEnvCards,
  saveCachedScores,
  sortNodeScores,
  testAll as runTestAll,
  testOne,
  type RunnerHooks,
  type RunnerProgress,
} from "../lib/runner";

export type TestMode = "current" | "all";

/** 进度文案拆成「阶段」+「节点名」。节点名统一交由 CSS 在超长时做省略号截断（title 保留全名）。 */
export function splitProgressDisplay(p: RunnerProgress | null): {
  phase: string;
  node?: string;
} | null {
  if (!p?.text) return null;
  const embedded = /^(.*?)（(.+?)）$/u.exec(p.text);
  if (embedded) {
    return { phase: embedded[1]!.trimEnd(), node: embedded[2] };
  }
  const prefixed =
    /^(正在切换到|正在检测|正在切回原先节点)[：:\s]+(.+?)(?:…|\.{3})?$/u.exec(
      p.text,
    );
  if (prefixed) {
    return { phase: prefixed[1]!, node: prefixed[2]!.trim() };
  }
  const cleanPhase = p.text.replace(/(?:…|\.{3})$/u, "").trimEnd();
  if (p.testingNode && !cleanPhase.includes(p.testingNode)) {
    return { phase: cleanPhase, node: p.testingNode };
  }
  return { phase: p.text };
}

export function useRunnerController({
  connection,
  nodes,
  clientId,
  forceMock,
  onRefresh,
}: {
  connection: ConnectionState;
  nodes: ProxyNode[];
  clientId: ClientId | null;
  forceMock: boolean;
  onRefresh?: (explicitClientId?: ClientId | null) => Promise<unknown> | void;
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
  /** 从本地缓存恢复的上次检测时间（本轮尚未跑新检测时显示） */
  const [cachedNodesAt, setCachedNodesAt] = useState<string | null>(null);
  const [cachedEnvAt, setCachedEnvAt] = useState<string | null>(null);
  const [allConfirmOpen, setAllConfirmOpen] = useState(false);
  const [switchHint, setSwitchHint] = useState<string | null>(null);
  const [restoreError, setRestoreError] = useState<string | null>(null);
  const [testingNode, setTestingNode] = useState<string | null>(null);

  const abortAllRef = useRef(false);
  const autoSniffedRef = useRef(false);
  const cacheScope = cacheScopeFor(
    clientId,
    forceMock || connection.usingMock,
  );
  const prevScopeRef = useRef<string | null>(cacheScope);

  // 切换客户端或 Mock 模式时重置当轮状态，以便干净回填对应缓存
  useEffect(() => {
    if (prevScopeRef.current === cacheScope) return;
    prevScopeRef.current = cacheScope;
    setNodeScores([]);
    setEnvCards(ENV_PLACEHOLDERS);
    setEnvOpen(false);
    setNodeTestOk(false);
    setEnvCheckOk(false);
    setCachedNodesAt(null);
    setCachedEnvAt(null);
  }, [cacheScope]);

  // 获取到节点列表后，若本轮尚未跑过实时检测，自动回填最近一次缓存结果
  useEffect(() => {
    if (!cacheScope || nodes.length === 0) return;
    const cached = loadCachedResults(
      cacheScope,
      nodes.map((n) => n.name),
    );
    if (!cached) return;
    if (
      !running &&
      !nodeTestOk &&
      nodeScores.length === 0 &&
      cached.scores.length > 0
    ) {
      setNodeScores(cached.scores);
      setCachedNodesAt(cached.scoresUpdatedAt);
    }
    // B方案：环境泄漏检查每次新打开软件保持折叠状态，不自动展开旧卡片，避免误导为当前实时环境
  }, [
    cacheScope,
    nodes,
    running,
    nodeTestOk,
    nodeScores.length,
    envRunning,
    envCheckOk,
  ]);

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

  const refreshAndGate = useCallback(
    async (explicitClientId?: ClientId | null) => {
      const startedAt = Date.now();
      setRefreshing(true);
      setGate(null);
      try {
        const next = await onRefresh?.(explicitClientId);
        const conn =
          next && typeof next === "object" && "status" in (next as object)
            ? (next as ConnectionState)
            : connection;
        const g = await runLightGate(conn);
        // 保证用户肉眼能感知到「同步中…」的反馈，避免 50ms 瞬回看起来像没动静
        const elapsed = Date.now() - startedAt;
        if (elapsed < 380) {
          await new Promise((r) => setTimeout(r, 380 - elapsed));
        }
        setGate(g);
      } finally {
        setRefreshing(false);
      }
    },
    [onRefresh, connection],
  );

  // 打开软件后自动嗅探一次正在运行的客户端并拉出节点 + 轻门禁校验
  useEffect(() => {
    if (autoSniffedRef.current) return;
    autoSniffedRef.current = true;
    void refreshAndGate();
  }, [refreshAndGate]);

  const ensureGate = async (): Promise<GateResult> => {
    setProgress({ text: "测试条件检查中…" });
    const g = await runLightGate(connection);
    setGate(g);
    // 不置 null：避免测全部接上预检前闪空白/「连…」；由 runner 覆盖或结束时清空
    return g;
  };

  const buildHooks = (resetSubset?: ReadonlySet<string>): RunnerHooks => ({
    onProgress: setProgress,
    onNodeCards: setNodeCards,
    onUpsertNodeCard: upsertNodeCard,
    onReport: setReport,
    onUpsertScore: (score) => {
      setCachedNodesAt(null);
      saveCachedScores(cacheScope, [score]);
      setNodeScores((prev) => {
        const next = prev.filter((x) => x.nodeName !== score.nodeName);
        next.push(score);
        return sortNodeScores(next);
      });
    },
    onScores: (list) => {
      if (list.length > 0) {
        setCachedNodesAt(null);
        saveCachedScores(cacheScope, list);
      }
      setNodeScores((prev) => mergeScoreList(prev, list, resetSubset));
    },
    onHint: setSwitchHint,
    onRestoreError: setRestoreError,
    onGate: setGate,
    onEnvCards: (cards) => {
      setEnvCards(cards);
      saveCachedEnvCards(cacheScope, cards);
    },
    onUpsertEnvCard: upsertEnvCard,
  });

  const testAll = async (targetNodes: ProxyNode[], isFiltered: boolean) => {
    if (targetNodes.length === 0) return;
    const subsetNames = isFiltered
      ? new Set(targetNodes.map((n) => n.name))
      : undefined;
    abortAllRef.current = false;
    setNodeTestOk(false);
    setCachedNodesAt(null);
    setRunning(true);
    try {
      const ok = await runTestAll(
        {
          connection,
          nodes: targetNodes,
          forceMock,
          mixedPort: mixedPortNum,
          ensureGate,
          shouldAbort: () => abortAllRef.current,
        },
        buildHooks(subsetNames),
      );
      if (ok) setNodeTestOk(true);
    } finally {
      setRunning(false);
      abortAllRef.current = false;
    }
  };

  const runEnvCheck = async () => {
    setEnvCheckOk(false);
    setCachedEnvAt(null);
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

  const openConfirmAll = () => {
    setMode("all");
    setRestoreError(null);
    setAllConfirmOpen(true);
  };

  const confirmAll = (targetNodes: ProxyNode[], isFiltered: boolean) => {
    setAllConfirmOpen(false);
    void testAll(targetNodes, isFiltered);
  };

  const onAbortAll = () => {
    abortAllRef.current = true;
    setProgress({ text: "正在停止…", testingNode: undefined });
  };

  // 单独测某个节点：完整深测（runNodeDiagnostics）；非当前节点临时切换后切回。
  const testOneNode = async (
    node: ProxyNode,
    onExpand?: (name: string) => void,
  ) => {
    if (running) return;
    setMode("current");
    setNodeTestOk(false);
    setCachedNodesAt(null);
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
        onExpand?.(node.name);
        setNodeTestOk(true);
      }
    } finally {
      setRunning(false);
      setTestingNode(null);
      setProgress(null);
    }
  };

  // 步骤③完成判定：三种检测路径（环境检查/测全部/测单节点）任一产出过结果
  const envDone =
    envCards.length > 0 &&
    envCards.every((c) => c.level !== "unknown" && c.level !== "running");
  const detectionDone = nodeScores.length > 0 || envDone;

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

  const cachedEnvTimeText = formatCachedTime(cachedEnvAt);
  const cachedNodesTimeText = formatCachedTime(cachedNodesAt);

  return {
    cacheScope,
    mixedPortNum,
    nodeCards,
    envCards,
    running,
    refreshing,
    mode,
    gate,
    nodeScores,
    envOpen,
    envRunning,
    envCheckOk,
    nodeTestOk,
    allConfirmOpen,
    setAllConfirmOpen,
    switchHint,
    restoreError,
    testingNode,
    detectionDone,
    progressDisplay,
    barPct,
    cachedEnvTimeText,
    cachedNodesTimeText,
    refreshAndGate,
    runEnvCheck,
    openConfirmAll,
    confirmAll,
    onAbortAll,
    testOneNode,
  };
}
