/**
 * 测单个节点：完整深测（runNodeDiagnostics）。
 * 非当前节点时临时切换、测完切回。无 React。
 */
import { runNodeDiagnostics } from "../egress";
import {
  confirmTrafficLeaf,
  findSelectorGroup,
  nodeTestBlockReason,
  refreshClashMode,
  resolveSelectorSnapshot,
  restoreProxy,
  switchProxy,
  type SelectorSnapshot,
} from "../mihomo";
import { scoreNodeFromCards } from "../score";
import { NODE_PLACEHOLDERS, asRunning } from "./placeholders";
import { planRestore, restoreErrorFor } from "./restore";
import type { RunnerHooks, TestOneContext } from "./types";

export async function testOne(
  ctx: TestOneContext,
  hooks: RunnerHooks,
): Promise<boolean> {
  const { node, connection, forceMock, mixedPort, ensureGate, shouldAbort } = ctx;
  const config = connection.config;
  let snap: SelectorSnapshot | null = null;
  let didSwitch = false;
  const aborted = () => shouldAbort?.() === true;
  const stopClean = (): boolean => {
    hooks.onHint?.(null);
    hooks.onProgress(null);
    return false;
  };

  hooks.onRestoreError?.(null);

  try {
    const g = await ensureGate();
    if (aborted()) return stopClean();
    hooks.onGate?.(g);
    if (!g.ok) return false;

    const realSwitch = !!config && !connection.usingMock && !forceMock;
    if (realSwitch && config) {
      try {
        await refreshClashMode(config, { force: true });
      } catch (err) {
        if (aborted()) return stopClean();
        throw err;
      }
      if (aborted()) return stopClean();
      const blocked = nodeTestBlockReason(config.mode);
      if (blocked) {
        hooks.onGate?.({ ok: false, message: blocked });
        hooks.onHint?.(blocked);
        return false;
      }
    }

    const isCurrent = node.name === connection.currentProxy;
    // 「使用中」必须就是流量组的有效叶子。对不上就照样切换，不能把另一个出口的结果记在这个节点上。
    let alreadyThere = false;
    if (isCurrent && realSwitch && config) {
      try {
        alreadyThere = await confirmTrafficLeaf(config, node.name);
      } catch (err) {
        if (aborted()) return stopClean();
        throw err;
      }
      if (aborted()) return stopClean();
    }
    if (realSwitch && config && !alreadyThere) {
      if (aborted()) return stopClean();
      hooks.onProgress({
        text: `正在切换到 ${node.name}…`,
        testingNode: node.name,
        stage: "switch",
      });
      try {
        snap = await resolveSelectorSnapshot(config, connection.currentProxy);
        if (aborted()) return stopClean();
        const group =
          snap?.group ?? (await findSelectorGroup(config, node.name));
        if (aborted()) return stopClean();
        if (!group) {
          hooks.onHint?.("找不到可切换的策略组，没法单独测这个节点。");
          return false;
        }
        didSwitch = true;
        const ok = await switchProxy(config, group, node.name);
        if (aborted()) return stopClean();
        if (!ok) {
          hooks.onHint?.("切换失败，没法测这个节点。");
          return false;
        }
      } catch (err) {
        if (aborted()) return stopClean();
        throw err;
      }
      await new Promise((r) => setTimeout(r, 250));
      if (aborted()) return stopClean();
    }

    if (aborted()) return stopClean();
    hooks.onProgress({
      text: `正在检测 ${node.name}…`,
      testingNode: node.name,
      stage: "test",
    });
    hooks.onNodeCards(asRunning(NODE_PLACEHOLDERS));

    const r = await runNodeDiagnostics(hooks.onUpsertNodeCard, {
      mixedPort,
      mihomoConfig: config,
    });
    hooks.onReport?.(r);
    hooks.onNodeCards(r.cards);
    const scored = scoreNodeFromCards(node.name, r.cards, r.ranAt);
    hooks.onUpsertScore?.(scored);
    return !aborted();
  } catch (err) {
    if (aborted()) return stopClean();
    hooks.onHint?.(err instanceof Error ? err.message : String(err));
    return false;
  } finally {
    // 与 testAll 同一套切回决策/文案。切过但没记下原节点时也必须报错，不可静默。
    const plan = planRestore(didSwitch, snap);
    if (plan.kind === "restore" && config) {
      hooks.onProgress({
        text: `正在切回原先节点：${plan.now}…`,
        stage: "restore",
      });
      const restored = await restoreProxy(config, {
        group: plan.group,
        now: plan.now,
        ...(plan.restoreChain ? { restoreChain: plan.restoreChain } : {}),
      });
      hooks.onRestoreError?.(restoreErrorFor(plan, restored));
    } else if (plan.kind !== "none") {
      hooks.onRestoreError?.(restoreErrorFor(plan, false));
    } else {
      // 本轮无需切回：清掉上一轮红字，仍挂在「测全部」旁同一处
      hooks.onRestoreError?.(null);
    }
    hooks.onProgress(null);
  }
}
