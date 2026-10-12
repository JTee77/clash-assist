/**
 * 测单个节点：完整深测（runNodeDiagnostics）。
 * 非当前节点时临时切换、测完切回。无 React。
 */
import { runNodeDiagnostics } from "../egress";
import {
  findSelectorGroup,
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
  const { node, connection, forceMock, mixedPort, ensureGate } = ctx;
  const config = connection.config;
  let snap: SelectorSnapshot | null = null;
  let didSwitch = false;

  hooks.onRestoreError?.(null);

  try {
    const g = await ensureGate();
    hooks.onGate?.(g);
    if (!g.ok) return false;

    const isCurrent = node.name === connection.currentProxy;
    if (!isCurrent && config && !connection.usingMock && !forceMock) {
      hooks.onProgress({
        text: `正在切换到 ${node.name}…`,
        testingNode: node.name,
        stage: "switch",
      });
      snap = await resolveSelectorSnapshot(config, connection.currentProxy);
      const group =
        (await findSelectorGroup(config, node.name)) ?? snap?.group;
      if (!group) {
        hooks.onHint?.("找不到可切换的策略组，没法单独测这个节点。");
        return false;
      }
      const ok = await switchProxy(config, group, node.name);
      if (!ok) {
        hooks.onHint?.("切换失败，没法测这个节点。");
        return false;
      }
      didSwitch = true;
      await new Promise((r) => setTimeout(r, 250));
    }

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
    return true;
  } catch (err) {
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
