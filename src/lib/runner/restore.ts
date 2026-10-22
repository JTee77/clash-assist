/**
 * 切回原节点：决策与文案的唯一来源（GUI / CLI / scripts/simulate-restore-fail.mjs 共用）。
 *
 * 约束：本文件只能有 `import type`（运行时零依赖），
 * 以便 Node（≥22.18，自带去类型）直接 import 做无 GUI 自检。
 */
import type { SelectorSnapshot } from "../mihomo";

/** 结束时该怎么切回。 */
export type RestorePlan =
  /** 没切过节点：什么都不用做，也不报错。 */
  | { kind: "none" }
  /** 切过且记下了原先节点：应调用 restoreProxy。 */
  | {
      kind: "restore";
      group: string;
      now: string;
      restoreChain?: Array<{ group: string; now: string }>;
    }
  /** 切过但没记下原先节点：无法自动切回，必须报错。 */
  | { kind: "missing_snapshot" };

export function planRestore(
  didSwitch: boolean,
  snapshot: SelectorSnapshot | null | undefined,
): RestorePlan {
  if (!didSwitch) return { kind: "none" };
  if (snapshot?.group && snapshot.now) {
    return {
      kind: "restore",
      group: snapshot.group,
      now: snapshot.now,
      ...(snapshot.restoreChain ? { restoreChain: snapshot.restoreChain } : {}),
    };
  }
  return { kind: "missing_snapshot" };
}

/** 切回失败文案（GUI 红字 / CLI data.restoreError 同一句）。 */
export function restoreFailedMessage(originalNode?: string | null): string {
  return originalNode
    ? `没能切回原先节点「${originalNode}」。`
    : "没能切回原先节点。";
}

/**
 * 根据计划与切回结果得出 restoreError。
 * - none → null
 * - restore 成功 → null；失败 → 带节点名的报错
 * - missing_snapshot → 不带节点名的报错（restored 参数被忽略：根本没法切回）
 */
export function restoreErrorFor(
  plan: RestorePlan,
  restored: boolean,
): string | null {
  switch (plan.kind) {
    case "none":
      return null;
    case "restore":
      return restored ? null : restoreFailedMessage(plan.now);
    case "missing_snapshot":
      return restoreFailedMessage(null);
  }
}
