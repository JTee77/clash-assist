import { useEffect, useMemo, useRef, useState } from "react";
import {
  classifyByRegion,
  shortRegionName,
  type ProxyNode,
} from "../lib/mihomo";
import { starRank, type NodeScoreResult } from "../lib/score";

/** 宽屏筛选栏最多直接平铺几个地区胶囊；超出部分收进紧凑「更多地区」下拉，确保单行不折行。 */
export const MAX_INLINE_REGIONS = 6;

/** 瀑布流列数：单卡固定 298px + 9px 列距，1–8 列封顶。
 * 余量 ≈ main 左右 padding + tray 左右 pad。
 * 1920 宽约 6 列；(vw-72)/307。 */
export function colCountFor(vw: number): number {
  return Math.min(8, Math.max(1, Math.floor((vw - 72) / 307)));
}

/**
 * 节点三桶排序（纯函数）：
 * - 已测出成绩时：已测活节点在前（按星级/总分降序） → 未测节点居中（当前节点优先，其余保持原序） → 不可用节点垫底
 * - 未测过任何节点时：当前节点置顶，其余保持订阅原序
 */
export function orderNodesByScore(
  nodes: readonly ProxyNode[],
  currentProxy: string | null,
  nodeScores: readonly NodeScoreResult[],
  scoreByName: ReadonlyMap<string, NodeScoreResult>,
): ProxyNode[] {
  const ordered = [...nodes];
  if (nodeScores.length > 0) {
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
      if (a.name === currentProxy) return -1;
      if (b.name === currentProxy) return 1;
      return 0;
    });
    return ordered;
  }
  if (currentProxy) {
    const i = ordered.findIndex((n) => n.name === currentProxy);
    if (i > 0) {
      const [row] = ordered.splice(i, 1);
      ordered.unshift(row);
    }
  }
  return ordered;
}

/** 按地区与搜索词过滤节点（纯函数）。 */
export function filterOrderedNodes(
  orderedNodes: readonly ProxyNode[],
  regionFilter: string | null,
  searchQuery: string,
): ProxyNode[] {
  const trimmedQuery = searchQuery.trim().toLowerCase();
  if (regionFilter === null && trimmedQuery.length === 0) {
    return [...orderedNodes];
  }
  return orderedNodes.filter((n) => {
    if (regionFilter !== null && n.region !== regionFilter) return false;
    if (trimmedQuery.length > 0) {
      const hay =
        `${n.name} ${n.type} ${n.region} ${shortRegionName(n.region)}`.toLowerCase();
      if (!hay.includes(trimmedQuery)) return false;
    }
    return true;
  });
}

export function useNodeFilter({
  nodes,
  currentProxy,
  nodeScores,
  cacheScope,
}: {
  nodes: ProxyNode[];
  currentProxy: string | null;
  nodeScores: NodeScoreResult[];
  cacheScope: string | null;
}) {
  const [regionFilter, setRegionFilter] = useState<string | null>(null);
  const [searchQuery, setSearchQuery] = useState("");
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [colCount, setColCount] = useState(() =>
    typeof window === "undefined" ? 2 : colCountFor(window.innerWidth),
  );
  const prevScopeRef = useRef<string | null>(cacheScope);

  // 切换客户端或 Mock 模式时重置筛选与展开状态
  useEffect(() => {
    if (prevScopeRef.current === cacheScope) return;
    prevScopeRef.current = cacheScope;
    setRegionFilter(null);
    setSearchQuery("");
    setExpanded(new Set());
  }, [cacheScope]);

  // 瀑布流列数：Tauri 的 webview 宽度 = 窗口宽度，直接用 window.innerWidth + resize 事件
  useEffect(() => {
    const onResize = () => setColCount(colCountFor(window.innerWidth));
    onResize();
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, []);

  const scoreByName = useMemo(() => {
    const m = new Map<string, NodeScoreResult>();
    for (const s of nodeScores) m.set(s.nodeName, s);
    return m;
  }, [nodeScores]);

  const orderedNodes = useMemo(
    () => orderNodesByScore(nodes, currentProxy, nodeScores, scoreByName),
    [nodes, currentProxy, nodeScores, scoreByName],
  );

  const regionGroups = useMemo(() => classifyByRegion(nodes), [nodes]);

  // 订阅更新后若原选中的地区已不存在，自动回到「全部」
  useEffect(() => {
    if (
      regionFilter !== null &&
      !regionGroups.some(([reg]) => reg === regionFilter)
    ) {
      setRegionFilter(null);
    }
  }, [regionFilter, regionGroups]);

  const trimmedQuery = searchQuery.trim().toLowerCase();
  const isFiltered = regionFilter !== null || trimmedQuery.length > 0;

  const filteredNodes = useMemo(
    () =>
      isFiltered
        ? filterOrderedNodes(orderedNodes, regionFilter, searchQuery)
        : orderedNodes,
    [orderedNodes, isFiltered, regionFilter, searchQuery],
  );

  const inlineRegions = useMemo(
    () =>
      regionGroups.length <= MAX_INLINE_REGIONS
        ? regionGroups
        : regionGroups.slice(0, MAX_INLINE_REGIONS - 1),
    [regionGroups],
  );
  const overflowRegions = useMemo(
    () =>
      regionGroups.length <= MAX_INLINE_REGIONS
        ? []
        : regionGroups.slice(MAX_INLINE_REGIONS - 1),
    [regionGroups],
  );
  const overflowNodeCount = useMemo(
    () => overflowRegions.reduce((sum, [, list]) => sum + list.length, 0),
    [overflowRegions],
  );
  const overflowActive = useMemo(
    () =>
      regionFilter !== null &&
      overflowRegions.some(([reg]) => reg === regionFilter),
    [regionFilter, overflowRegions],
  );

  const nodeColumns: ProxyNode[][] = useMemo(() => {
    const cols: ProxyNode[][] = Array.from({ length: colCount }, () => []);
    filteredNodes.forEach((n, i) => cols[i % colCount].push(n));
    return cols;
  }, [filteredNodes, colCount]);

  const allExpanded =
    filteredNodes.length > 0 &&
    filteredNodes.every((n) => expanded.has(n.name));

  const toggleExpand = (name: string) =>
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(name)) next.delete(name);
      else next.add(name);
      return next;
    });

  const expandNode = (name: string) =>
    setExpanded((prev) => new Set(prev).add(name));

  const toggleExpandAll = () =>
    setExpanded((prev) => {
      const next = new Set(prev);
      if (allExpanded) {
        for (const n of filteredNodes) next.delete(n.name);
      } else {
        for (const n of filteredNodes) next.add(n.name);
      }
      return next;
    });

  const resetFilter = () => {
    setRegionFilter(null);
    setSearchQuery("");
  };

  return {
    regionFilter,
    setRegionFilter,
    searchQuery,
    setSearchQuery,
    resetFilter,
    expanded,
    allExpanded,
    toggleExpand,
    expandNode,
    toggleExpandAll,
    scoreByName,
    orderedNodes,
    regionGroups,
    isFiltered,
    filteredNodes,
    inlineRegions,
    overflowRegions,
    overflowNodeCount,
    overflowActive,
    nodeColumns,
  };
}
