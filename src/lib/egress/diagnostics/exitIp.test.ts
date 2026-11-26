import { describe, it, expect, beforeEach, vi } from "vitest";

// 锁住出口 IP 的多源降级 + 「机房判定为关键词推断（非权威）」的语义：
// v0.1.9 只把 geojs.io 追加为第三冗余源（提升 ipwho.is/ip.sb 双双 429 时的容灾），
// hosting 仍只在关键词命中时为 true；没命中或组织名为空是 null /「未判定」，不再默认住宅。
const { fetchTextViaProxy } = vi.hoisted(() => ({ fetchTextViaProxy: vi.fn() }));
vi.mock("../fetchVia", () => ({ fetchTextViaProxy }));

import { formatCardBrief } from "../cardBrief";
import { scoreNodeFromCards } from "../../score/nodeScore";
import { exitIpCard, fetchExitIp, inferHosting } from "./exitIp";

const IPWHO = "https://ipwho.is/";
const IPSB = "https://api.ip.sb/geoip";
const GEOJS = "https://get.geojs.io/v1/ip/geo.json";

type Served = Record<string, string | null>;

function serve(map: Served) {
  fetchTextViaProxy.mockImplementation((url: string) => {
    const body = map[url];
    if (body === undefined || body === null) {
      return Promise.resolve({ ok: false, status: 0, text: "", via: "rust" as const });
    }
    return Promise.resolve({ ok: true, status: 200, text: body, via: "rust" as const });
  });
}

function calledUrls(): string[] {
  return fetchTextViaProxy.mock.calls.map((c) => c[0] as string);
}

beforeEach(() => {
  fetchTextViaProxy.mockReset();
});

describe("fetchExitIp 多源降级（v0.1.9 第三冗余源）", () => {
  it("ipwho.is 首选成功即用，不再打后续源", async () => {
    serve({
      [IPWHO]: JSON.stringify({
        success: true,
        ip: "5.6.7.8",
        country: "Germany",
        country_code: "DE",
        connection: { org: "Hetzner Online GmbH", isp: "Hetzner" },
      }),
    });
    const info = await fetchExitIp(null);
    expect(info.ip).toBe("5.6.7.8");
    expect(info.hosting).toBe(true); // Hetzner 命中关键词
    expect(info.hostingInferred).toBe(true); // 明确是推断，不是权威字段
    expect(info.ipTypeLabel).toContain("疑似机房");
    expect(calledUrls()).toEqual([IPWHO]); // 短路，未碰后两源
  });

  it("前两源失败时回落第三源 geojs.io（验证接线与顺序）", async () => {
    serve({
      [GEOJS]: JSON.stringify({
        ip: "5.6.7.9",
        country: "United States",
        country_code: "US",
        organization: "AS7922 Comcast Cable Communications",
      }),
    });
    const info = await fetchExitIp(7897);
    expect(info.ip).toBe("5.6.7.9");
    expect(info.hosting).toBeNull(); // Comcast 不含机房关键词，也不因此是住宅
    expect(info.ipTypeLabel).toBe("未判定");
    expect(info.ipTypeLabel).not.toContain("住宅");
    // 顺序：ipwho → ip.sb → geojs
    expect(calledUrls()).toEqual([IPWHO, IPSB, GEOJS]);
  });

  it("ipwho success:false 视为该源失败，回落下一源", async () => {
    serve({
      [IPWHO]: JSON.stringify({ success: false, ip: "should-be-ignored" }),
      [IPSB]: JSON.stringify({
        ip: "1.1.1.1",
        country: "China",
        country_code: "CN",
        organization: "China Telecom",
      }),
    });
    const info = await fetchExitIp(null);
    expect(info.ip).toBe("1.1.1.1");
    expect(calledUrls()).toEqual([IPWHO, IPSB]);
  });

  it("IT7 / Cluster Logic 与空组织名标未判定，不加住宅分", async () => {
    const cases = [
      {
        name: "IT7",
        connection: { org: "IT7 Networks Inc", isp: "IT7 Networks Inc" },
        ip: "23.106.130.20",
      },
      {
        name: "Cluster Logic",
        connection: { org: "Cluster Logic Inc", isp: "Cluster Logic Inc" },
        ip: "96.45.186.222",
      },
      { name: "空组织名", connection: { org: "", isp: null }, ip: "203.0.113.10" },
      { name: "缺字段", connection: {}, ip: "203.0.113.11" },
    ];
    for (const row of cases) {
      serve({
        [IPWHO]: JSON.stringify({
          success: true,
          ip: row.ip,
          country: "Canada",
          country_code: "CA",
          connection: row.connection,
        }),
      });
      const info = await fetchExitIp(null);
      expect(info.hosting, row.name).toBeNull();
      expect(info.ipTypeLabel, row.name).toBe("未判定");
      expect(info.ipTypeLabel, row.name).not.toMatch(/住宅|家宽/);
      const card = exitIpCard(info);
      expect(card.level, row.name).toBe("unknown");
      expect(card.conclusion, row.name).toContain("未判定");
      expect(card.conclusion, row.name).not.toMatch(/住宅|家宽/);
      expect(card.metrics?.hosting, row.name).toBeNull();
      expect(formatCardBrief(card), row.name).toBe("CA · 未判定");
      const scored = scoreNodeFromCards(row.name, [card]);
      const exit = scored.breakdown.find((b) => b.key === "exit");
      expect(exit?.score, row.name).not.toBe(95);
      expect(exit?.note, row.name).not.toContain("出口更像住宅线路");
    }
  });

  it("Hetzner 仍判疑似机房，卡片降为 warn", async () => {
    serve({
      [IPWHO]: JSON.stringify({
        success: true,
        ip: "5.6.7.8",
        country: "Germany",
        country_code: "DE",
        connection: { org: "Hetzner Online GmbH", isp: "Hetzner" },
      }),
    });
    const info = await fetchExitIp(null);
    const card = exitIpCard(info);
    expect(card.level).toBe("warn");
    expect(card.conclusion).toContain("疑似机房(DCH)");
    expect(formatCardBrief(card)).toBe("DE · 机房");
    const exit = scoreNodeFromCards("hetzner", [card]).breakdown.find((b) => b.key === "exit");
    expect(exit?.score).toBe(55);
    expect(exit?.note).toContain("机房");
  });

  it("三源全失败 → 诚实返回空结果（不编造 IP）", async () => {
    serve({});
    const info = await fetchExitIp(null);
    expect(info.ip).toBeNull();
    expect(info.hosting).toBeNull();
    expect(info.ipTypeLabel).toBe("--");
    expect(calledUrls()).toEqual([IPWHO, IPSB, GEOJS]);
  });
});

describe("inferHosting 关键词推断（证据不足不是住宅）", () => {
  it("命中机房/云厂商关键词判 hosting", () => {
    expect(inferHosting("Hetzner Online GmbH")).toBe(true);
    expect(inferHosting("AS16509 Amazon.com, Inc.")).toBe(true);
    expect(inferHosting(undefined, "OVH SAS")).toBe(true);
  });

  it("IT7 / Cluster Logic 没命中关键词，返回 null 而不是住宅", () => {
    expect(inferHosting("IT7 Networks Inc")).toBeNull();
    expect(inferHosting("IT7 Networks Inc", "IT7 Networks Inc")).toBeNull();
    expect(inferHosting("Cluster Logic Inc")).toBeNull();
    expect(inferHosting(undefined, "Cluster Logic Inc")).toBeNull();
  });

  it("普通运营商名字也不写成住宅", () => {
    expect(inferHosting("Comcast Cable", "Xfinity")).toBeNull();
    expect(inferHosting("China Telecom")).toBeNull();
  });

  it("组织名和 ISP 都空 → null", () => {
    expect(inferHosting(null, undefined, "")).toBeNull();
    expect(inferHosting(null, null)).toBeNull();
    expect(inferHosting("", "  ")).toBeNull();
  });
});
