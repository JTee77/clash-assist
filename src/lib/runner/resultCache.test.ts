import { beforeEach, describe, expect, it } from "vitest";
import {
  RESULT_CACHE_TTL_MS,
  cacheScopeFor,
  formatCachedTime,
  loadCachedResults,
  mergeScoreList,
  saveCachedEnvCards,
  saveCachedScores,
} from "./resultCache";
import type { NodeScoreResult } from "../score";
import type { CheckCard } from "../egress/types";

function makeStorage(): Storage {
  const map = new Map<string, string>();
  return {
    get length() {
      return map.size;
    },
    clear() {
      map.clear();
    },
    getItem(k: string) {
      return map.has(k) ? map.get(k)! : null;
    },
    key(i: number) {
      return [...map.keys()][i] ?? null;
    },
    removeItem(k: string) {
      map.delete(k);
    },
    setItem(k: string, v: string) {
      map.set(k, String(v));
    },
  };
}

function makeScore(
  nodeName: string,
  stars: NodeScoreResult["stars"],
  totalScore: number,
): NodeScoreResult {
  return {
    nodeName,
    stars,
    totalScore,
    blurb: "测试评语",
    breakdown: [],
    cards: [],
    ranAt: "2026-10-18T06:00:00.000Z",
  };
}

describe("resultCache", () => {
  beforeEach(() => {
    (globalThis as Record<string, unknown>).__TEST_STORAGE__ = true;
    Object.defineProperty(globalThis, "localStorage", {
      value: makeStorage(),
      configurable: true,
      writable: true,
    });
  });

  it("isolates scopes between real client and mock", () => {
    expect(cacheScopeFor("verge", false)).toBe("verge");
    expect(cacheScopeFor("verge", true)).toBe("mock");
    expect(cacheScopeFor(null, false)).toBeNull();

    saveCachedScores("verge", [makeScore("HK 01", 4.5, 88)], "2026-10-18T06:00:00.000Z");
    saveCachedScores("mock", [makeScore("Mock 01", 3, 65)], "2026-10-18T06:00:00.000Z");

    const verge = loadCachedResults("verge", ["HK 01"], Date.parse("2026-10-18T07:00:00.000Z"));
    expect(verge?.scores.map((s) => s.nodeName)).toEqual(["HK 01"]);

    const mock = loadCachedResults("mock", ["Mock 01"], Date.parse("2026-10-18T07:00:00.000Z"));
    expect(mock?.scores.map((s) => s.nodeName)).toEqual(["Mock 01"]);
  });

  it("merges partial node test results and filters to current subscription nodes", () => {
    const t0 = "2026-10-18T06:00:00.000Z";
    const t1 = "2026-10-18T06:10:00.000Z";
    saveCachedScores("verge", [makeScore("HK 01", 4, 80), makeScore("Old Node", 5, 95)], t0);
    saveCachedScores("verge", [makeScore("JP 01", 4.5, 87)], t1);

    const loaded = loadCachedResults(
      "verge",
      ["HK 01", "JP 01", "US 01"],
      Date.parse("2026-10-18T07:00:00.000Z"),
    );
    expect(loaded?.scoresUpdatedAt).toBe(t1);
    expect(loaded?.scores.map((s) => s.nodeName)).toEqual(["JP 01", "HK 01"]);
  });

  it("persists completed envCards alongside node scores and expires after TTL", () => {
    const t0 = "2026-10-18T06:00:00.000Z";
    const envCards: CheckCard[] = [
      { id: "dns-leak", title: "DNS 解析器", level: "pass", conclusion: "正常" },
    ];
    saveCachedScores("verge", [makeScore("HK 01", 4, 80)], t0);
    saveCachedEnvCards("verge", envCards, t0);

    const fresh = loadCachedResults("verge", ["HK 01"], Date.parse(t0) + 1000);
    expect(fresh?.scores).toHaveLength(1);
    expect(fresh?.envCards).toHaveLength(1);

    const expired = loadCachedResults(
      "verge",
      ["HK 01"],
      Date.parse(t0) + RESULT_CACHE_TTL_MS + 1000,
    );
    expect(expired).toBeNull();
  });

  it("strips the Windows adapterHint before caching (PR #51 S-3)", () => {
    const t0 = "2026-10-18T06:00:00.000Z";
    const envCards: CheckCard[] = [
      {
        id: "dns-leak",
        title: "DNS 解析器",
        level: "pass",
        conclusion: "正常",
        process: "配置解析器：10.0.0.1（source win-adapter-default）",
        adapterHint: "- 以太网 #7 [Intel] type 6 up dns 10.0.0.1",
      },
    ];
    saveCachedEnvCards("verge", envCards, t0);
    const node = makeScore("HK 01", 4, 80);
    node.cards = [{ ...envCards[0], id: "x" }];
    saveCachedScores("verge", [node], t0);

    const raw = localStorage.getItem("clash-assist.results.v1.verge") ?? "";
    expect(raw).not.toContain("adapterHint");
    expect(raw).not.toContain("以太网 #7");

    const fresh = loadCachedResults("verge", ["HK 01"], Date.parse(t0) + 1000);
    expect(fresh?.envCards[0].adapterHint).toBeUndefined();
    expect(fresh?.envCards[0].process).toContain("win-adapter-default");
    // 内存里的卡片不被改动（只影响写入缓存的副本）。
    expect(envCards[0].adapterHint).toContain("以太网");
  });

  it("mergeScoreList preserves untested region scores during filtered subset reset", () => {
    const prev = [
      makeScore("HK 01", 4.5, 88),
      makeScore("JP 01", 3.5, 72),
    ];
    const subsetReset = mergeScoreList(prev, [], new Set(["JP 01"]));
    expect(subsetReset.map((s) => s.nodeName)).toEqual(["HK 01"]);

    const subsetDone = mergeScoreList(
      subsetReset,
      [makeScore("JP 01", 5, 94)],
      new Set(["JP 01"]),
    );
    expect(subsetDone.map((s) => s.nodeName)).toEqual(["JP 01", "HK 01"]);
  });

  it("formats same-day and cross-day timestamps concisely", () => {
    const now = new Date(2026, 9, 18, 15, 30, 0);
    const sameDay = new Date(2026, 9, 18, 9, 5, 0).toISOString();
    const prevDay = new Date(2026, 9, 17, 21, 40, 0).toISOString();
    expect(formatCachedTime(sameDay, now)).toBe("09:05");
    expect(formatCachedTime(prevDay, now)).toBe("10-17 21:40");
  });
});
