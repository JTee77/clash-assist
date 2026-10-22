/**
 * Resolve which node the user is actually on from the /proxies map, and locate
 * the authoritative Selector group(s) needed to temporarily switch to a target
 * leaf node and cleanly restore afterwards.
 *
 * Preferred groups (Proxy, GLOBAL, 节点选择, …) are followed through nested
 * Selector / URLTest / Fallback / LoadBalance chains until a real leaf.
 * DIRECT, REJECT, PASS, and other group names are never returned.
 *
 * After a subscription switch the main picker is often renamed (「🚀 节点选择」)
 * and policy groups (NETFLIX, AI, Telegram, …) disagree on `now`. An exact
 * preferred name that chains to a leaf still wins. Otherwise a Selector whose
 * name looks like the main picker (选择 / proxy, including an emoji prefix)
 * wins — even when it only lists other groups and the real node is further
 * down the chain. If there is no such name, the Selector with the most real
 * leaves wins. Null only when that is still ambiguous, or the main picker
 * itself is sitting on DIRECT.
 */
import { IGNORE_PROXY_TYPES, JUNK_NAME_KEYWORDS, type ProxyInfo } from "./types";

const GROUP_TYPES = new Set(["Selector", "URLTest", "Fallback", "LoadBalance"]);

/** User-facing selector groups, most authoritative first. */
export const PREFER_GROUP_NAMES = [
  "Proxy",
  "GLOBAL",
  "proxy",
  "SELECT",
  "节点选择",
  "手动选择",
  "自动选择",
];

const MAX_HOPS = 8;

function isJunkName(name: string): boolean {
  if (name.startsWith("PASS") || name.startsWith("REJECT")) return true;
  return JUNK_NAME_KEYWORDS.some((k) => name.includes(k));
}

/** A real leaf proxy: exists, not a group/pseudo type (Direct/Reject/Pass…), not junk. */
function isRealLeaf(
  proxies: Record<string, ProxyInfo>,
  name: string | undefined,
): name is string {
  if (!name) return false;
  const p = proxies[name];
  if (!p) return false;
  if (IGNORE_PROXY_TYPES.has(p.type)) return false;
  return !isJunkName(name);
}

function isSelectorType(p: ProxyInfo | undefined): boolean {
  return (p?.type ?? "").toLowerCase() === "selector";
}

function isGroupType(p: ProxyInfo | undefined): boolean {
  if (!p?.type) return false;
  if (GROUP_TYPES.has(p.type)) return true;
  const lower = p.type.toLowerCase();
  return (
    lower === "selector" ||
    lower === "urltest" ||
    lower === "fallback" ||
    lower === "loadbalance"
  );
}

/**
 * 3 = main picker (节点选择 / proxy / …), 2 = some other「选择」group,
 * 1 = name mentions 节点 (often a region subgroup), 0 = neither.
 */
export function selectorNameRank(name: string): number {
  const lower = name.toLowerCase();
  if (
    name.includes("节点选择") ||
    name.includes("手动选择") ||
    name.includes("选择节点") ||
    name.includes("代理选择") ||
    name.includes("选择代理") ||
    lower.includes("proxy") ||
    lower === "select"
  ) {
    return 3;
  }
  if (name.includes("选择")) return 2;
  if (name.includes("节点")) return 1;
  return 0;
}

/** Walk `now` through nested groups until a real leaf. Cycles and DIRECT stop as null. */
function followToLeaf(
  proxies: Record<string, ProxyInfo>,
  start: string,
): string | null {
  const seen = new Set<string>();
  let current: string | undefined = start;
  for (let hop = 0; hop < MAX_HOPS; hop++) {
    if (!current || seen.has(current)) return null;
    seen.add(current);
    if (isRealLeaf(proxies, current)) return current;
    const group: ProxyInfo | undefined = proxies[current];
    if (!group || !isGroupType(group)) return null;
    current = group.now;
  }
  return null;
}

function countRealLeaves(
  proxies: Record<string, ProxyInfo>,
  group: ProxyInfo,
): number {
  let n = 0;
  for (const name of group.all ?? []) {
    if (isRealLeaf(proxies, name)) n++;
  }
  return n;
}

interface Cand {
  group: string;
  leaf: string;
  leaves: number;
  rank: number;
  selector: boolean;
}

/** Highest rank, then most real leaves. Different leaves at that peak → null. */
function uniqueCandByRank(cands: Cand[]): Cand | null {
  return uniqueCandBy(cands, (c) => [c.rank, c.leaves]);
}

/** Most real leaves, then higher name rank. Different leaves at that peak → null. */
function uniqueCandBySize(cands: Cand[]): Cand | null {
  return uniqueCandBy(cands, (c) => [c.leaves, c.rank]);
}

function uniqueCandBy(
  cands: Cand[],
  key: (c: Cand) => [number, number],
): Cand | null {
  if (cands.length === 0) return null;
  let best = key(cands[0]);
  for (const c of cands) {
    const k = key(c);
    if (k[0] > best[0] || (k[0] === best[0] && k[1] > best[1])) best = k;
  }
  const top = cands.filter((c) => {
    const k = key(c);
    return k[0] === best[0] && k[1] === best[1];
  });
  const leaf = top[0].leaf;
  return top.every((c) => c.leaf === leaf) ? top[0] : null;
}

function resolvePrimaryEntry(
  proxies: Record<string, ProxyInfo>,
): { group: string; leaf: string } | null {
  for (const g of PREFER_GROUP_NAMES) {
    if (!proxies[g]) continue;
    const leaf = followToLeaf(proxies, g);
    if (leaf) return { group: g, leaf };
  }

  const strong: Cand[] = [];
  let sawStrongSelector = false;
  const resolved: Cand[] = [];

  for (const [name, p] of Object.entries(proxies)) {
    if (!isGroupType(p)) continue;
    const rank = selectorNameRank(name);
    const selector = isSelectorType(p);
    if (selector && rank >= 2) sawStrongSelector = true;
    const leaf = followToLeaf(proxies, name);
    if (!leaf) continue;
    const cand: Cand = {
      group: name,
      leaf,
      leaves: countRealLeaves(proxies, p),
      rank,
      selector,
    };
    resolved.push(cand);
    if (selector && rank >= 2) strong.push(cand);
  }

  // A renamed main picker (「🚀 节点选择」) owns the answer. If every such
  // selector is on DIRECT, do not borrow a niche policy group's node.
  if (sawStrongSelector) {
    const hit = uniqueCandByRank(strong);
    return hit ? { group: hit.group, leaf: hit.leaf } : null;
  }

  if (resolved.length === 0) return null;
  const only = resolved[0].leaf;
  if (resolved.every((c) => c.leaf === only)) {
    const firstSel = resolved.find((c) => c.selector) ?? resolved[0];
    return { group: firstSel.group, leaf: firstSel.leaf };
  }

  const selectors = resolved.filter((c) => c.selector);
  const hit = uniqueCandBySize(selectors.length > 0 ? selectors : resolved);
  return hit ? { group: hit.group, leaf: hit.leaf } : null;
}

export function resolveCurrentProxy(
  proxies: Record<string, ProxyInfo>,
): string | null {
  return resolvePrimaryEntry(proxies)?.leaf ?? null;
}

export type SelectorSnapshot = {
  group: string;
  /** 该策略组当前选中的节点（或子组）名 */
  now: string | null;
  /**
   * 当存在嵌套分组或规则分流组时，记录所有受影响策略组的原始状态，
   * 以便切回时按逆序 100% 恢复用户原来的分组结构。
   */
  restoreChain?: Array<{ group: string; now: string }>;
};

/**
 * 在 proxies 字典里解析主控 Selector 策略组快照（纯函数，供 client.ts 与单测共用）。
 *
 * 规则：
 * 1. 绝不在规则模式下盲选停在 DIRECT/REJECT 的 `GLOBAL`。
 * 2. 优先取与 `resolveCurrentProxy` 同一权威主 Selector（如 `Proxy`、`🔰 节点选择`、`🚀 节点选择` 或节点最多的主组）。
 * 3. 若主组通过嵌套子组（如 `🔰 节点选择` -> `♻️ 自动选择` 或 `🇭🇰 香港节点`）指向叶子，
 *    将整条 Selector 链记入 `restoreChain`，确保测完后即便切过主组也能恢复回原来的子组。
 */
export function resolveSelectorSnapshotFromMap(
  proxies: Record<string, ProxyInfo>,
  nodeName?: string | null,
): SelectorSnapshot | null {
  const isSwitchable = (p: ProxyInfo | undefined) => {
    if (!p) return false;
    const t = (p.type || "").toLowerCase();
    return t === "selector" || t === "urltest" || t === "fallback";
  };

  // 收集从根组沿着 now 向下走的所有可切换组链条
  const buildChain = (rootGroup: string): Array<{ group: string; now: string }> => {
    const chain: Array<{ group: string; now: string }> = [];
    const seen = new Set<string>();
    let cur: string | undefined = rootGroup;
    for (let hop = 0; hop < MAX_HOPS; hop++) {
      if (!cur || seen.has(cur)) break;
      seen.add(cur);
      const p: ProxyInfo | undefined = proxies[cur];
      if (!p || !isGroupType(p)) break;
      if (isSelectorType(p) && p.now) {
        chain.push({ group: cur, now: p.now });
      }
      cur = p.now;
    }
    return chain;
  };

  // 1) 先用权威主组识别算法找当前真正生效的顶层策略组
  const primary = resolvePrimaryEntry(proxies);
  if (primary) {
    const p = proxies[primary.group];
    // 若顶层本身是 URLTest（例如「自动选择」），找引用它的外层主 Selector
    let rootGroup = primary.group;
    if (!isSelectorType(p)) {
      for (const [gName, gInfo] of Object.entries(proxies)) {
        if (isSelectorType(gInfo) && gInfo.now === primary.group) {
          rootGroup = gName;
          break;
        }
      }
    }
    const rootInfo = proxies[rootGroup];
    if (rootInfo && isSwitchable(rootInfo)) {
      const chain = buildChain(rootGroup);
      // 同时记录其它当前指向主组的二级分流策略组（如 🌍 国外媒体、🤖 人工智能、🎥 NETFLIX），
      // 防止部分机场把节点只放在地区组或专项组里时需要联动切换并恢复。
      const extras: Array<{ group: string; now: string }> = [];
      for (const [gName, gInfo] of Object.entries(proxies)) {
        if (gName === rootGroup || gName === "GLOBAL") continue;
        if (!isSelectorType(gInfo) || !gInfo.now) continue;
        if (chain.some((c) => c.group === gName)) continue;
        // 如果某个分流组当前直接停在某个真实节点（而非指向主组或直连/拦截），也纳入保护快照
        if (isRealLeaf(proxies, gInfo.now) && countRealLeaves(proxies, gInfo) > 0) {
          extras.push({ group: gName, now: gInfo.now });
        }
      }
      const fullChain = [...chain, ...extras];
      return {
        group: rootGroup,
        now: rootInfo.now ?? nodeName ?? null,
        ...(fullChain.length > 1 ? { restoreChain: fullChain } : {}),
      };
    }
  }

  // 2) 若指定了 nodeName，按权威度排序寻找包含该节点的 Selector（排除停在 DIRECT 的 GLOBAL）
  const scored: Array<{ name: string; score: number; leaves: number }> = [];
  for (const [name, p] of Object.entries(proxies)) {
    if (!isSwitchable(p)) continue;
    if (nodeName && !p.all?.includes(nodeName)) continue;
    if (!nodeName && !p.now) continue;

    // 规则模式下 GLOBAL.now === "DIRECT"，绝不优先选它当主切换组
    const isGlobalDirect =
      name === "GLOBAL" && (!p.now || p.now === "DIRECT" || p.now === "REJECT");
    const exactPreferIdx = PREFER_GROUP_NAMES.indexOf(name);
    const rank = selectorNameRank(name);
    const selBonus = isSelectorType(p) ? 100 : 0;
    const globalPenalty = isGlobalDirect ? -500 : 0;
    const preferBonus =
      exactPreferIdx >= 0 && !isGlobalDirect ? (20 - exactPreferIdx) * 10 : 0;
    const leaves = countRealLeaves(proxies, p);
    const score = selBonus + preferBonus + rank * 30 + globalPenalty;
    scored.push({ name, score, leaves });
  }

  scored.sort((a, b) => b.score - a.score || b.leaves - a.leaves);
  const best = scored[0]?.name;
  if (!best) return null;
  const now = proxies[best]?.now ?? nodeName ?? null;
  return { group: best, now };
}

/**
 * 给定目标节点 `targetNode`，在 `proxies` 中计算需要执行的切换步骤 `Array<{ group, name }>`。
 *
 * 兼容所有主流机场分组形态：
 * 1. **扁平主组**（`🔰 节点选择` 的 `all` 直接包含 `targetNode`）：
 *    直接切换 `[{ group: "🔰 节点选择", name: targetNode }]`。
 * 2. **地区/嵌套子组**（`🚀 节点选择` 的 `all` 只有 `["🇭🇰 香港节点", "🇺🇸 美国节点"]`，而 `🇺🇸 美国节点` 的 `all` 包含 `targetNode`）：
 *    返回两步：`[{ group: "🇺🇸 美国节点", name: targetNode }, { group: "🚀 节点选择", name: "🇺🇸 美国节点" }]`，
 *    确保主入口真正导流到该目标节点。
 * 3. **专项分流组覆盖**（例如测 AI / Netflix / Google 时，如果配置中存在独立的 `🤖 人工智能`、`🎥 NETFLIX`、`🌍 国外媒体` 等 Selector，
 *    且该组当前没有指向主组、而是独立选了别的节点，同时它的 `all` 包含主组或目标节点）：
 *    自动同步切换，防止主组切了但专项流量仍走旧节点。
 */
export function resolveSwitchPlanFromMap(
  proxies: Record<string, ProxyInfo>,
  targetNode: string,
  primaryGroupHint?: string | null,
): Array<{ group: string; name: string }> {
  const snap = resolveSelectorSnapshotFromMap(proxies, targetNode);
  const mainGroup = primaryGroupHint ?? snap?.group ?? null;

  const isSelector = (name: string) => isSelectorType(proxies[name]);

  // 能在最多 3 层 Selector 内把 group 切到 targetNode 的路径（从下往上切换）
  const findPathFromGroup = (
    startGroup: string,
  ): Array<{ group: string; name: string }> | null => {
    const g0 = proxies[startGroup];
    if (!g0 || !isSelector(startGroup)) return null;
    if (g0.all?.includes(targetNode)) {
      return [{ group: startGroup, name: targetNode }];
    }
    // 二级嵌套：startGroup -> childGroup (Selector) -> targetNode
    for (const child of g0.all ?? []) {
      const g1 = proxies[child];
      if (!g1 || !isSelector(child)) continue;
      if (g1.all?.includes(targetNode)) {
        return [
          { group: child, name: targetNode },
          { group: startGroup, name: child },
        ];
      }
      // 三级嵌套：startGroup -> child -> grandChild (Selector) -> targetNode
      for (const grand of g1.all ?? []) {
        const g2 = proxies[grand];
        if (!g2 || !isSelector(grand)) continue;
        if (g2.all?.includes(targetNode)) {
          return [
            { group: grand, name: targetNode },
            { group: child, name: grand },
            { group: startGroup, name: child },
          ];
        }
      }
    }
    return null;
  };

  if (mainGroup) {
    const plan = findPathFromGroup(mainGroup);
    if (plan) return plan;
  }

  // 若主组（或 hint）里找不到，遍历所有高优先级 Selector 寻找能切到 targetNode 的组
  const fallbackSnap = resolveSelectorSnapshotFromMap(proxies, targetNode);
  if (fallbackSnap?.group) {
    const plan = findPathFromGroup(fallbackSnap.group);
    if (plan) return plan;
    if (proxies[fallbackSnap.group]?.all?.includes(targetNode)) {
      return [{ group: fallbackSnap.group, name: targetNode }];
    }
  }

  return [];
}
