import { describe, expect, it } from "vitest";
import {
  colCountFor,
  filterOrderedNodes,
  orderNodesByScore,
} from "./useNodeFilter";
import type { ProxyNode } from "../lib/mihomo";
import type { NodeScoreResult } from "../lib/score";

function makeNode(name: string, region: string, type = "Trojan"): ProxyNode {
  return {
    name,
    region,
    type,
    raw: { name, type },
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
    blurb: "",
    breakdown: [],
    cards: [],
    ranAt: "2026-10-20T00:00:00Z",
  };
}

describe("useNodeFilter pure helpers", () => {
  it("colCountFor clamps columns between 1 and 8 based on viewport width", () => {
    expect(colCountFor(360)).toBe(1);
    expect(colCountFor(760)).toBe(2);
    expect(colCountFor(1920)).toBe(6);
    expect(colCountFor(3840)).toBe(8);
  });

  it("orderNodesByScore puts currentProxy first when no scores exist", () => {
    const nodes = [
      makeNode("HK 01", "中国香港"),
      makeNode("JP 01", "日本"),
      makeNode("US 01", "美国"),
    ];
    const ordered = orderNodesByScore(nodes, "JP 01", [], new Map());
    expect(ordered.map((n) => n.name)).toEqual(["JP 01", "HK 01", "US 01"]);
  });

  it("orderNodesByScore sorts into 3 buckets: live (by star/score) -> untested (current first) -> unavailable", () => {
    const nodes = [
      makeNode("Dead 01", "中国香港"),
      makeNode("Untested A", "日本"),
      makeNode("Untested Current", "日本"),
      makeNode("Live 4star", "新加坡"),
      makeNode("Live 5star", "美国"),
    ];
    const scores = [
      makeScore("Dead 01", "unavailable", 0),
      makeScore("Live 4star", 4, 80),
      makeScore("Live 5star", 5, 95),
    ];
    const scoreMap = new Map(scores.map((s) => [s.nodeName, s]));
    const ordered = orderNodesByScore(
      nodes,
      "Untested Current",
      scores,
      scoreMap,
    );
    expect(ordered.map((n) => n.name)).toEqual([
      "Live 5star",
      "Live 4star",
      "Untested Current",
      "Untested A",
      "Dead 01",
    ]);
  });

  it("filterOrderedNodes filters by region and search query (including shortRegionName)", () => {
    const nodes = [
      makeNode("🇭🇰 香港 IEPL 01", "中国香港", "Shadowsocks"),
      makeNode("🇯🇵 东京 BGP 01", "日本", "VLESS"),
      makeNode("🇦🇺 悉尼 01", "澳大利亚", "Trojan"),
    ];
    expect(filterOrderedNodes(nodes, "日本", "").map((n) => n.name)).toEqual([
      "🇯🇵 东京 BGP 01",
    ]);
    // Search by shortRegionName ("澳洲" for "澳大利亚")
    expect(filterOrderedNodes(nodes, null, "澳洲").map((n) => n.name)).toEqual([
      "🇦🇺 悉尼 01",
    ]);
    // Search by protocol + region
    expect(
      filterOrderedNodes(nodes, "中国香港", "shadow").map((n) => n.name),
    ).toEqual(["🇭🇰 香港 IEPL 01"]);
  });
});
