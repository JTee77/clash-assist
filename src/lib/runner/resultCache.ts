import type { CheckCard } from "../egress/types";
import { starRank, type NodeScoreResult } from "../score";

export const RESULT_CACHE_PREFIX = "clash-assist.results.v1.";
/** 缓存保留 7 天；超过则自动丢弃。 */
export const RESULT_CACHE_TTL_MS = 7 * 24 * 60 * 60 * 1000;

export type CachedResultEntry = {
  version: 1;
  scoresUpdatedAt: string | null;
  scores: NodeScoreResult[];
  envUpdatedAt: string | null;
  envCards: CheckCard[];
};

export function cacheScopeFor(
  clientId: string | null | undefined,
  isMock: boolean,
): string | null {
  if (isMock) return "mock";
  if (typeof clientId === "string" && clientId.trim()) return clientId.trim();
  return null;
}

function keyFor(scope: string): string {
  return `${RESULT_CACHE_PREFIX}${scope}`;
}

function isFresh(iso: string | null | undefined, nowMs: number): boolean {
  if (!iso) return false;
  const t = Date.parse(iso);
  return Number.isFinite(t) && t <= nowMs + 60_000 && nowMs - t <= RESULT_CACHE_TTL_MS;
}

export function sortNodeScores(list: NodeScoreResult[]): NodeScoreResult[] {
  return [...list].sort((a, b) => {
    const byStar = starRank(b.stars) - starRank(a.stars);
    if (byStar !== 0) return byStar;
    return b.totalScore - a.totalScore;
  });
}

/**
 * 合并节点评分列表：
 * - 当 incoming 为空且传了 resetNames（局部/筛选开测前清空该子集），只移除 resetNames 中的节点评分，保留其余节点评分。
 * - 当 incoming 非空，按 nodeName 覆盖合并并按星级/总分排序。
 */
export function mergeScoreList(
  prev: NodeScoreResult[],
  incoming: NodeScoreResult[],
  resetNames?: ReadonlySet<string>,
): NodeScoreResult[] {
  if (incoming.length === 0) {
    if (!resetNames) return [];
    return sortNodeScores(prev.filter((s) => !resetNames.has(s.nodeName)));
  }
  const byName = new Map<string, NodeScoreResult>();
  for (const s of prev) {
    if (!resetNames || !resetNames.has(s.nodeName)) {
      byName.set(s.nodeName, s);
    }
  }
  for (const s of incoming) {
    byName.set(s.nodeName, s);
  }
  return sortNodeScores([...byName.values()]);
}

function getStorage(): Storage | null {
  if (typeof window === "undefined" && !("__TEST_STORAGE__" in globalThis)) {
    return null;
  }
  try {
    return typeof localStorage !== "undefined" ? localStorage : null;
  } catch {
    return null;
  }
}

function readRawEntry(scope: string): CachedResultEntry | null {
  const ls = getStorage();
  if (!ls) return null;
  try {
    const raw = ls.getItem(keyFor(scope));
    if (!raw) return null;
    const parsed = JSON.parse(raw) as Partial<CachedResultEntry>;
    if (!parsed || parsed.version !== 1) return null;
    return {
      version: 1,
      scoresUpdatedAt:
        typeof parsed.scoresUpdatedAt === "string" ? parsed.scoresUpdatedAt : null,
      scores: Array.isArray(parsed.scores) ? parsed.scores : [],
      envUpdatedAt:
        typeof parsed.envUpdatedAt === "string" ? parsed.envUpdatedAt : null,
      envCards: Array.isArray(parsed.envCards) ? parsed.envCards : [],
    };
  } catch {
    return null;
  }
}

function writeRawEntry(scope: string, entry: CachedResultEntry): void {
  const ls = getStorage();
  if (!ls) return;
  try {
    ls.setItem(keyFor(scope), JSON.stringify(entry));
  } catch {
    /* ignore quota / storage errors */
  }
}

export function loadCachedResults(
  scope: string | null,
  validNodeNames?: Iterable<string>,
  nowMs = Date.now(),
): CachedResultEntry | null {
  if (!scope) return null;
  const entry = readRawEntry(scope);
  if (!entry) return null;

  const validSet = validNodeNames ? new Set(validNodeNames) : null;
  let scores: NodeScoreResult[] = [];
  let scoresUpdatedAt: string | null = null;
  if (isFresh(entry.scoresUpdatedAt, nowMs) && entry.scores.length > 0) {
    scores = sortNodeScores(
      entry.scores.filter(
        (s) =>
          s &&
          typeof s.nodeName === "string" &&
          (!validSet || validSet.has(s.nodeName)),
      ),
    );
    if (scores.length > 0) {
      scoresUpdatedAt = entry.scoresUpdatedAt;
    }
  }

  let envCards: CheckCard[] = [];
  let envUpdatedAt: string | null = null;
  if (
    isFresh(entry.envUpdatedAt, nowMs) &&
    entry.envCards.length > 0 &&
    entry.envCards.every(
      (c) => c && c.level !== "unknown" && c.level !== "running",
    )
  ) {
    envCards = entry.envCards;
    envUpdatedAt = entry.envUpdatedAt;
  }

  if (scores.length === 0 && envCards.length === 0) return null;
  return {
    version: 1,
    scoresUpdatedAt,
    scores,
    envUpdatedAt,
    envCards,
  };
}

export function saveCachedScores(
  scope: string | null,
  incoming: NodeScoreResult[],
  updatedAt = new Date().toISOString(),
): void {
  if (!scope || incoming.length === 0) return;
  const prev = readRawEntry(scope);
  const merged = mergeScoreList(prev?.scores ?? [], incoming);
  writeRawEntry(scope, {
    version: 1,
    scoresUpdatedAt: updatedAt,
    scores: merged,
    envUpdatedAt: prev?.envUpdatedAt ?? null,
    envCards: prev?.envCards ?? [],
  });
}

export function saveCachedEnvCards(
  scope: string | null,
  cards: CheckCard[],
  updatedAt = new Date().toISOString(),
): void {
  if (
    !scope ||
    cards.length === 0 ||
    !cards.every((c) => c && c.level !== "unknown" && c.level !== "running")
  ) {
    return;
  }
  const prev = readRawEntry(scope);
  writeRawEntry(scope, {
    version: 1,
    scoresUpdatedAt: prev?.scoresUpdatedAt ?? null,
    scores: prev?.scores ?? [],
    envUpdatedAt: updatedAt,
    envCards: cards,
  });
}

function pad2(n: number): string {
  return String(n).padStart(2, "0");
}

/**
 * 格式化缓存时间供 UI 紧凑展示：
 * - 当天：`HH:mm`（如 `14:32`）
 * - 跨天：`MM-DD HH:mm`（如 `10-17 19:05`）
 */
export function formatCachedTime(
  iso: string | null | undefined,
  now = new Date(),
): string {
  if (!iso) return "";
  const d = new Date(iso);
  if (!Number.isFinite(d.getTime())) return "";
  const hh = pad2(d.getHours());
  const mm = pad2(d.getMinutes());
  const sameDay =
    d.getFullYear() === now.getFullYear() &&
    d.getMonth() === now.getMonth() &&
    d.getDate() === now.getDate();
  if (sameDay) return `${hh}:${mm}`;
  const mo = pad2(d.getMonth() + 1);
  const da = pad2(d.getDate());
  return `${mo}-${da} ${hh}:${mm}`;
}
