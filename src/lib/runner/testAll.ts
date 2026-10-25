/**
 * 测全部节点：完整深测（runNodeDiagnostics），与测单个相同探针全集。
 * 含连通性预检、临时切换、切回；无 React。
 */
import { mapPool, runNodeDiagnostics } from "../egress";
import {
  findSelectorGroup,
  probeDelay,
  resolveSelectorSnapshot,
  restoreProxy,
  switchProxy,
  type ProxyNode,
  type SelectorSnapshot,
} from "../mihomo";
import {
  scoreDeadNode,
  scoreNodeFromCards,
  starRank,
  type GateResult,
  type NodeScoreResult,
} from "../score";
import { DELAY_URL, NODE_PLACEHOLDERS, asRunning } from "./placeholders";
import { planRestore, restoreErrorFor } from "./restore";
import type { RunnerHooks, TestAllContext } from "./types";

const CULL_CONCURRENCY = 12;
const CLIENT_DEAD_FRESH_MS = 10 * 60 * 1000;

function sortScores(list: NodeScoreResult[]): NodeScoreResult[] {
  return [...list].sort((a, b) => {
    const byStar = starRank(b.stars) - starRank(a.stars);
    if (byStar !== 0) return byStar;
    return b.totalScore - a.totalScore;
  });
}

export async function testAll(
  ctx: TestAllContext,
  hooks: RunnerHooks,
): Promise<boolean> {
  const { connection, nodes, forceMock, mixedPort, ensureGate, shouldAbort } =
    ctx;

  hooks.onScores?.([]);
  hooks.onHint?.(null);
  hooks.onRestoreError?.(null);

  let originalSnap: SelectorSnapshot | null = null;
  let didSwitch = false;
  const config = connection.config;

  try {
    const g = await ensureGate();
    hooks.onGate?.(g);
    if (!g.ok) return false;

    const list: ProxyNode[] = nodes.length
      ? nodes
      : connection.currentProxy
        ? [
            {
              name: connection.currentProxy,
              type: "Unknown",
              region: "未知",
              raw: { name: connection.currentProxy, type: "Unknown" },
            },
          ]
        : [];

    if (!list.length) {
      const fail: GateResult = {
        ok: false,
        message:
          "还没有读到节点列表。请先点「刷新节点」，确认VPN软件里已经加载了订阅。",
      };
      hooks.onGate?.(fail);
      return false;
    }

    const results: NodeScoreResult[] = [];
    const alive: ProxyNode[] = [];
    const canSwitch = !!config && !connection.usingMock && !forceMock;

    const upsertScore = (score: NodeScoreResult) => {
      const i = results.findIndex((r) => r.nodeName === score.nodeName);
      if (i >= 0) results[i] = score;
      else results.push(score);
      hooks.onUpsertScore?.(score);
    };

    if (canSwitch) {
      originalSnap = await resolveSelectorSnapshot(
        config,
        connection.currentProxy,
      );
      if (!originalSnap?.now && connection.currentProxy) {
        const group =
          (await findSelectorGroup(config, connection.currentProxy)) ??
          originalSnap?.group;
        if (group) {
          originalSnap = { group, now: connection.currentProxy };
        }
      }
    }

    const clientDead = list.filter((n) => {
      if (n.lastDelay !== 0 || !n.lastDelayAt) return false;
      const t = Date.parse(n.lastDelayAt.replace(/(\.\d{3})\d+/, "$1"));
      return Number.isFinite(t) && Date.now() - t <= CLIENT_DEAD_FRESH_MS;
    });
    for (const n of clientDead) {
      upsertScore(scoreDeadNode(n.name, "客户端最近测速失败，按不可用处理。"));
    }
    const toCheck = list.filter((n) => !clientDead.includes(n));

    // 不发 0/N：避免进度区尚未撑开时「连通性预检」被缩成「连…」
    hooks.onProgress({
      text: "正在预检连通性…", stage: "precheck",
      current: 0,
      total: toCheck.length,
      testingNode: undefined,
    });

    if (!canSwitch) {
      for (let i = 0; i < toCheck.length; i++) {
        if (shouldAbort()) break;
        const n = toCheck[i]!;
        hooks.onProgress({
          text: `连通性预检 ${i + 1}/${toCheck.length}`, stage: "precheck",
          current: i + 1,
          total: toCheck.length,
          testingNode: undefined,
        });
        if (i === toCheck.length - 1 && toCheck.length > 1) {
          upsertScore(
            scoreDeadNode(n.name, "演示：延迟探测失败，按不可用处理。"),
          );
        } else {
          alive.push(n);
        }
      }
    } else {
      let cullDone = 0;
      const cullOut = await mapPool(toCheck, CULL_CONCURRENCY, async (n) => {
        if (shouldAbort()) {
          return { n, delay: null as number | null, skipped: true };
        }
        let delay = await probeDelay(config!, n.name, DELAY_URL, 5000);
        if (delay == null && !shouldAbort()) {
          delay = await probeDelay(config!, n.name, DELAY_URL, 5000);
        }
        cullDone += 1;
        hooks.onProgress({
          text: `连通性预检 ${cullDone}/${toCheck.length}`, stage: "precheck",
          current: cullDone,
          total: toCheck.length,
          // 预检并发，不挂节点名（避免进度文案狂跳；阶段语完整显示）
          testingNode: undefined,
        });
        if (delay == null) {
          upsertScore(scoreDeadNode(n.name, "延迟探测失败，按不可用处理。"));
        }
        return { n, delay, skipped: false };
      });
      for (const row of cullOut) {
        if (row.skipped) continue;
        if (row.delay == null) continue;
        alive.push(row.n);
      }
    }

    // 正式检测从正在使用的节点开始，其余保持原顺序。预检仍并发，不排先后。
    // 正在用的节点若已在预检里判死，不在 alive 里，就不会被提前再测一遍。
    const inUse = connection.currentProxy;
    if (inUse) {
      const at = alive.findIndex((n) => n.name === inUse);
      if (at > 0) {
        const [row] = alive.splice(at, 1);
        if (row) alive.unshift(row);
      }
    }

    if (canSwitch && !shouldAbort() && originalSnap?.group && originalSnap.now) {
      for (let i = 0; i < alive.length; i++) {
        if (shouldAbort()) break;
        const n = alive[i]!;
        hooks.onProgress({
          // 节点名交给 UI（testingNode）单独 ellipsis，阶段「检测 N/M」完整显示
          text: `检测 ${i + 1}/${alive.length}`, stage: "test",
          current: i + 1,
          total: alive.length,
          testingNode: n.name,
        });
        const group =
          originalSnap.group || (await findSelectorGroup(config!, n.name));
        if (!group) {
          upsertScore(
            scoreDeadNode(n.name, "找不到可切换的策略组，没法检测这个节点。"),
          );
          continue;
        }
        const ok = await switchProxy(config!, group, n.name);
        if (!ok) {
          upsertScore(scoreDeadNode(n.name, "切换失败，没法检测这个节点。"));
          continue;
        }
        didSwitch = true;
        await new Promise((r) => setTimeout(r, 250));
        if (shouldAbort()) break;
        hooks.onNodeCards(asRunning(NODE_PLACEHOLDERS));
        const r = await runNodeDiagnostics(hooks.onUpsertNodeCard, {
          mixedPort,
          mihomoConfig: config,
        });
        hooks.onReport?.(r);
        hooks.onNodeCards(r.cards);
        upsertScore(scoreNodeFromCards(n.name, r.cards, r.ranAt));
      }
    } else if (canSwitch) {
      hooks.onHint?.(
        "连上了VPN软件，但读不到当前选中的节点或策略组，没法安全地临时切换。只检测当前节点。",
      );
      hooks.onProgress({
        text: "正在检测当前节点（无法安全切换）…", stage: "test",
        testingNode: connection.currentProxy ?? undefined,
      });
      hooks.onNodeCards(asRunning(NODE_PLACEHOLDERS));
      const r = await runNodeDiagnostics(hooks.onUpsertNodeCard, {
        mixedPort,
        mihomoConfig: config,
      });
      hooks.onReport?.(r);
      hooks.onNodeCards(r.cards);
      const currentName = connection.currentProxy ?? "当前节点";
      upsertScore(scoreNodeFromCards(currentName, r.cards, r.ranAt));
      for (const n of alive) {
        if (n.name === currentName) continue;
        upsertScore(
          scoreDeadNode(
            n.name,
            "没法切换到该节点做检测（读不到策略组或当前选中）。",
          ),
        );
      }
    } else {
      for (let i = 0; i < alive.length; i++) {
        if (shouldAbort()) break;
        const n = alive[i]!;
        hooks.onProgress({
          text: `检测 ${i + 1}/${alive.length}（演示）`, stage: "test",
          current: i + 1,
          total: alive.length,
          testingNode: undefined,
        });
        hooks.onNodeCards(asRunning(NODE_PLACEHOLDERS));
        const r = await runNodeDiagnostics(hooks.onUpsertNodeCard, {
          mixedPort,
          mihomoConfig: config,
        });
        hooks.onReport?.(r);
        hooks.onNodeCards(r.cards);
        upsertScore(scoreNodeFromCards(n.name, r.cards, r.ranAt));
      }
    }

    hooks.onScores?.(sortScores(results));
    hooks.onProgress(null);
    return !shouldAbort();
  } catch (err) {
    const msg = err instanceof Error ? err.message : String(err);
    hooks.onProgress(null);
    hooks.onGate?.({ ok: false, message: `测全部节点时出错：${msg}` });
    return false;
  } finally {
    // 切回：决策与文案统一走 ./restore（GUI 红字 / CLI restoreError 同一句）。
    // restoreError 与 hint 分开：hint 只放过程提示，不再重复切回报错。
    const plan = planRestore(didSwitch, originalSnap);
    if (plan.kind === "restore" && config) {
      hooks.onProgress({
        text: `正在切回原先节点：${plan.now}…`, stage: "restore",
        testingNode: undefined,
      });
      const restored = await restoreProxy(config, {
        group: plan.group,
        now: plan.now,
        ...(plan.restoreChain ? { restoreChain: plan.restoreChain } : {}),
      });
      hooks.onRestoreError?.(restoreErrorFor(plan, restored));
      hooks.onProgress(null);
    } else if (plan.kind !== "none") {
      // missing_snapshot，或切过节点却没有 config（canSwitch 要求 config，理论上不可达）：按失败报。
      hooks.onRestoreError?.(restoreErrorFor(plan, false));
    }
  }
}
