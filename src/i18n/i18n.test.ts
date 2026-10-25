import { describe, expect, it } from "vitest";
import { langFromTag } from "./lang";
import { zhHans } from "./locales/zh-Hans";
import { zhHant } from "./locales/zh-Hant";
import { en } from "./locales/en";
import { EN_RULES } from "./enRules";
import { loadHant, toEnglish, toHant } from "./dynamic";
import { splitProgressDisplay } from "../pages/HomePage";
import { REGION_RULES, shortRegionName } from "../lib/mihomo/regions";
import { ENV_PLACEHOLDERS, NODE_PLACEHOLDERS } from "../lib/runner/placeholders";
import { scoreDeadNode, scoreNodeFromCards } from "../lib/score";

describe("splitProgressDisplay", () => {
  it("splits embedded parenthesized node name", () => {
    expect(splitProgressDisplay({ text: "检测 2/9（🇭🇰 香港 IEPL 01）" })).toEqual({
      phase: "检测 2/9",
      node: "🇭🇰 香港 IEPL 01",
    });
  });

  it("splits single-node and restore progress prefixes without keeping trailing ellipsis", () => {
    expect(
      splitProgressDisplay({ text: "正在切换到 🇭🇰 香港 IEPL 01…", testingNode: "🇭🇰 香港 IEPL 01" }),
    ).toEqual({
      phase: "正在切换到",
      node: "🇭🇰 香港 IEPL 01",
    });
    expect(
      splitProgressDisplay({ text: "正在检测 🇭🇰 香港 IEPL 01…", testingNode: "🇭🇰 香港 IEPL 01" }),
    ).toEqual({
      phase: "正在检测",
      node: "🇭🇰 香港 IEPL 01",
    });
    expect(
      splitProgressDisplay({ text: "正在切回原先节点：🇭🇰 香港 IEPL 01…" }),
    ).toEqual({
      phase: "正在切回原先节点",
      node: "🇭🇰 香港 IEPL 01",
    });
  });

  it("preserves non-node status messages intact", () => {
    expect(splitProgressDisplay({ text: "正在检测当前节点（无法安全切换）…" })).toEqual({
      phase: "正在检测当前节点（无法安全切换）…",
    });
    expect(splitProgressDisplay({ text: "测试条件检查中…" })).toEqual({
      phase: "测试条件检查中…",
    });
  });
});

describe("langFromTag", () => {
  it.each([
    ["zh-Hans-CN", "zh-Hans"],
    ["zh-CN", "zh-Hans"],
    ["zh", "zh-Hans"],
    ["zh-Hant-TW", "zh-Hant"],
    ["zh-TW", "zh-Hant"],
    ["zh_HK", "zh-Hant"],
    ["zh-MO", "zh-Hant"],
    ["zh-Hans-HK", "zh-Hans"],
    ["en-US", "en"],
    ["ja-JP", "en"],
    ["", "en"],
  ])("%s → %s", (tag, want) => expect(langFromTag(tag)).toBe(want));
});

describe("locale dictionaries", () => {
  it("have identical keys and placeholders", () => {
    for (const dict of [zhHant, en]) {
      expect(Object.keys(dict).sort()).toEqual(Object.keys(zhHans).sort());
      for (const k of Object.keys(zhHans) as (keyof typeof zhHans)[]) {
        const ph = (s: string) => (s.match(/\{\w+\}/g) ?? []).sort();
        expect(ph(dict[k])).toEqual(ph(zhHans[k]));
      }
    }
  });
  it("en has no CJK; zh-Hant has no common simplified-only characters", () => {
    for (const v of Object.values(en)) expect(v).not.toMatch(/[\u4e00-\u9fff]/);
    for (const v of Object.values(zhHant)) expect(v).not.toMatch(/[软节点检测获显设关闭这们网络实发现]/);
  });
});

describe("dynamic English", () => {
  it.each([
    ["尚未检测", "Not tested"],
    ["连通性", "Connectivity"],
    ["3/4 个境外探测点能通（约 120 ms）", "3/4 overseas probes reachable (~120 ms)"],
    ["整体不错：下行约 50 Mbps，够快。3 项较顺、1 项不行（共 4 项）。", "Good overall: ~50 Mbps down, fast enough. 3 OK, 1 failing (of 4)."],
    ["检测 2/9", "Testing 2/9"],
    ["流媒体：通 · US", "Streaming: OK · US"],
    ["大约 88 ms（3/3 次中位）（不稳定）", "~88 ms (median of 3/3) (unstable)"],
  ])("%s", (zh, want) => expect(toEnglish(zh)).toBe(want));

  it("translates multi-line process text line by line", () => {
    const zh = "第 1 次：120 ms（HTTP 204）\n第 2 次：失败（HTTP 超时 · 3000 ms）\n边界：同 URL 连续抽样，不是面板延迟。";
    expect(toEnglish(zh)).toBe(
      "Run 1: 120 ms (HTTP 204)\nRun 2: failed (HTTP timeout · 3000 ms)\nScope: consecutive samples of one URL, not the client panel's latency.",
    );
  });

  it("leaves non-Chinese text untouched", () => {
    expect(toEnglish("HK-1 · 42 ms")).toBe("HK-1 · 42 ms");
  });

  it("translates all region names and short region names without residual CJK", () => {
    const allRegions = [...REGION_RULES.map(([r]) => r), "其他地区"];
    for (const reg of allRegions) {
      const fullEn = toEnglish(reg);
      const shortEn = toEnglish(shortRegionName(reg));
      expect(fullEn, `region ${reg}`).not.toMatch(/[\u4e00-\u9fff]/);
      expect(shortEn, `shortRegion ${reg}`).not.toMatch(/[\u4e00-\u9fff]/);
    }
  });

  it("translates all EN_RULES templates, placeholders, and score blurbs with zero CJK leak", () => {
    for (const [zhPattern, enTarget] of EN_RULES) {
      expect(enTarget, `EN_RULES target for: ${zhPattern}`).not.toMatch(/[\u4e00-\u9fff]/);
      const sample = zhPattern.replace(/\{(\d+)\}/g, (_, i) => `ARG${i}`);
      const out = toEnglish(sample);
      expect(out, `toEnglish(${sample})`).not.toMatch(/[\u4e00-\u9fff]/);
    }

    for (const c of [...NODE_PLACEHOLDERS, ...ENV_PLACEHOLDERS]) {
      expect(toEnglish(c.title)).not.toMatch(/[\u4e00-\u9fff]/);
      expect(toEnglish(c.conclusion)).not.toMatch(/[\u4e00-\u9fff]/);
    }

    const dead = scoreDeadNode("HK 01", "延迟探测失败，按不可用处理。");
    expect(toEnglish(dead.blurb)).not.toMatch(/[\u4e00-\u9fff]/);
    for (const b of dead.breakdown) {
      expect(toEnglish(b.label)).not.toMatch(/[\u4e00-\u9fff]/);
      expect(toEnglish(b.note)).not.toMatch(/[\u4e00-\u9fff]/);
    }

    const scored = scoreNodeFromCards("US 01", NODE_PLACEHOLDERS);
    expect(toEnglish(scored.blurb)).not.toMatch(/[\u4e00-\u9fff]/);
    for (const b of scored.breakdown) {
      expect(toEnglish(b.label)).not.toMatch(/[\u4e00-\u9fff]/);
      expect(toEnglish(b.note)).not.toMatch(/[\u4e00-\u9fff]/);
    }
  });
});

describe("dynamic zh-Hant", () => {
  it("uses Taiwan phrasing", async () => {
    await loadHant();
    // 「刷新」经 opencc + HANT_FIXES 落到台湾用语「重新整理」，
    // 与繁中按钮文案 `home.getNodes: 重新整理節點` 逐字一致。
    expect(toHant("请先打开并连上你的VPN软件，再点「刷新节点」")).toBe("請先開啟並連上你的VPN軟體，再點「重新整理節點」");
  });
});
