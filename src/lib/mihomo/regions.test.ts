import { describe, expect, it } from "vitest";
import { classifyByRegion, detectRegion, detectRegions } from "./regions";

describe("detectRegion & classifyByRegion", () => {
  it("correctly classifies all nodes from the user screenshot (airport prefix + alphanumeric codes)", () => {
    const screenshotNodes = [
      { name: "香港高速3-HK22 香港奈飞解锁", expected: "中国香港" },
      { name: "kingfast-JP1", expected: "日本" },
      { name: "kingfast-JP2", expected: "日本" },
      { name: "kingfast-US1", expected: "美国" },
      { name: "香港高速3-HK15", expected: "中国香港" },
      { name: "香港高速3-HK16", expected: "中国香港" },
      { name: "香港高速3-HK17", expected: "中国香港" },
      { name: "香港高速3-HK20", expected: "中国香港" },
      { name: "香港高速3-HK21", expected: "中国香港" },
      { name: "香港高速3-HK24", expected: "中国香港" },
      { name: "香港高速3-JP80🇯🇵", expected: "日本" },
      { name: "香港高速3-JP81🇯🇵", expected: "日本" },
      { name: "香港高速3-JP90🇯🇵", expected: "日本" },
      { name: "香港高速3-德国2🇩🇪", expected: "德国" },
      { name: "香港高速3-德国🇩🇪", expected: "德国" },
      { name: "香港高速3-新加坡1", expected: "新加坡" },
      { name: "香港高速3-新加坡2", expected: "新加坡" },
      { name: "香港高速3-法国3🇫🇷", expected: "法国" },
      { name: "香港高速3-法国🇫🇷", expected: "法国" },
      { name: "香港高速3-美国01 美区奈飞解锁", expected: "美国" },
      { name: "香港高速3-美国02 美区奈飞解锁", expected: "美国" },
      { name: "香港高速3-美国1", expected: "美国" },
      { name: "香港高速3-美国12 0.2x", expected: "美国" },
      { name: "香港高速3-美国13 0.2x", expected: "美国" },
      { name: "香港高速3-美国2", expected: "美国" },
      { name: "香港高速3-美国81", expected: "美国" },
      { name: "香港高速3-美国82 奈飞", expected: "美国" },
      { name: "香港高速3-香港1", expected: "中国香港" },
    ];

    for (const { name, expected } of screenshotNodes) {
      expect(detectRegion(name), `single node: ${name}`).toBe(expected);
    }

    const grouped = new Map(classifyByRegion(screenshotNodes));
    expect(grouped.get("中国香港")?.length).toBe(8);
    expect(grouped.get("美国")?.length).toBe(9);
    expect(grouped.get("日本")?.length).toBe(5);
    expect(grouped.get("德国")?.length).toBe(2);
    expect(grouped.get("新加坡")?.length).toBe(2);
    expect(grouped.get("法国")?.length).toBe(2);
    expect(grouped.get("其他地区")).toBeUndefined();
  });

  it("Scenario 1: ignores cross-region streaming/AI unlock remarks when primary node region exists", () => {
    expect(detectRegion("🇸🇬 新加坡 01 | 解锁美区 Netflix")).toBe("新加坡");
    expect(detectRegion("🇸🇬 新加坡 02 | 美区 Netflix")).toBe("新加坡");
    expect(detectRegion("🇭🇰 香港 IEPL 02 (日区 TikTok / 台湾巴哈)")).toBe("中国香港");
    expect(detectRegion("香港 03 - 送中节点")).toBe("中国香港");
    expect(detectRegion("JP-01 [OpenAI 美区]")).toBe("日本");
    expect(detectRegion("香港高速3-美国01 美区奈飞解锁")).toBe("美国");
    // Fallback when node has ONLY unlock description
    expect(detectRegion("美区奈飞解锁 01")).toBe("美国");
    expect(detectRegion("香港 Netflix 专线")).toBe("中国香港");
  });

  it("Scenario 2: preserves single-region packages without stripping genuine region prefix", () => {
    const hkPackage = [
      { name: "香港BGP-01" },
      { name: "香港BGP-02" },
      { name: "香港BGP-03" },
      { name: "香港BGP-04" },
      { name: "香港BGP-05 (美区解锁)" },
    ];
    const groups = new Map(classifyByRegion(hkPackage));
    expect(groups.get("中国香港")?.length).toBe(5);
    expect(groups.get("美国")).toBeUndefined();
  });

  it("Scenario 3: resolves domestic ingress compounds, transit arrows, and transit remarks", () => {
    // Domestic ingress + landing compounds
    expect(detectRegion("深港 IEPL 01")).toBe("中国香港");
    expect(detectRegion("广港 IPLC 02")).toBe("中国香港");
    expect(detectRegion("沪日专线 02")).toBe("日本");
    expect(detectRegion("沪美 GIA 01")).toBe("美国");
    expect(detectRegion("京德 BGP 01")).toBe("德国");
    expect(detectRegion("深新 01")).toBe("新加坡");
    // Explicit landing overrides domestic line compound regardless of order
    expect(detectRegion("深港IEPL-日本01")).toBe("日本");
    expect(detectRegion("日本01-深港IEPL")).toBe("日本");
    // Directional arrows take destination
    expect(detectRegion("香港 -> 美国 01")).toBe("美国");
    expect(detectRegion("HK → JP 专线")).toBe("日本");
    expect(detectRegion("深圳 -> 香港 -> 新加坡 01")).toBe("新加坡");
    // Bracketed / clause transit remarks do not override landing country
    expect(detectRegion("美国 01 (香港中转)")).toBe("美国");
    expect(detectRegion("英国伦敦 [经由香港]")).toBe("英国");
    expect(detectRegion("日本 02 (深圳入口)")).toBe("日本");
    expect(detectRegion("新加坡 01 | 香港中转")).toBe("新加坡");
  });

  it("Scenario 4: supports IATA datacenter codes & code+digits without false positives on English words", () => {
    // IATA 3-letter codes
    expect(detectRegion("LAX-01")).toBe("美国");
    expect(detectRegion("SJC-02")).toBe("美国");
    expect(detectRegion("NRT-01")).toBe("日本");
    expect(detectRegion("KIX-01")).toBe("日本");
    expect(detectRegion("ICN-01")).toBe("韩国");
    expect(detectRegion("SIN-01")).toBe("新加坡");
    expect(detectRegion("TPE-01")).toBe("中国台湾");
    expect(detectRegion("FRA-01")).toBe("德国");
    expect(detectRegion("LHR-01")).toBe("英国");
    expect(detectRegion("AMS-01")).toBe("荷兰");
    expect(detectRegion("SYD-01")).toBe("澳大利亚");
    expect(detectRegion("YYZ-01")).toBe("加拿大");

    // Lowercase / uppercase code + digits
    expect(detectRegion("us01.node")).toBe("美国");
    expect(detectRegion("jp1-bgp")).toBe("日本");
    expect(detectRegion("hk_iepl_03")).toBe("中国香港");
    expect(detectRegion("sg2")).toBe("新加坡");

    // Must NOT false-trigger on English words ending with code letters + digits
    expect(detectRegion("Node1")).toBe("其他地区");
    expect(detectRegion("Plus1")).toBe("其他地区");
    expect(detectRegion("Main01")).toBe("其他地区");
    expect(detectRegion("Login-01")).toBe("其他地区");
    expect(detectRegion("CentOS-1")).toBe("其他地区");
    expect(detectRegion("macOS")).toBe("其他地区");
    expect(detectRegion("Built-in")).toBe("其他地区");
    expect(detectRegion("Plug-in")).toBe("其他地区");
    expect(detectRegion("All-in-One")).toBe("其他地区");
    expect(detectRegion("my-node")).toBe("其他地区");
  });

  it("Scenario 5: supports Traditional Chinese region & city names", () => {
    expect(detectRegion("東京 01")).toBe("日本");
    expect(detectRegion("大阪 02")).toBe("日本");
    expect(detectRegion("臺灣 01")).toBe("中国台湾");
    expect(detectRegion("台灣 台北 02")).toBe("中国台湾");
    expect(detectRegion("韓國首爾")).toBe("韩国");
    expect(detectRegion("美國洛杉磯")).toBe("美国");
    expect(detectRegion("聖荷西 GIA")).toBe("美国");
    expect(detectRegion("英國倫敦")).toBe("英国");
    expect(detectRegion("德國法蘭克福")).toBe("德国");
    expect(detectRegion("法國巴黎")).toBe("法国");
    expect(detectRegion("荷蘭阿姆斯特丹")).toBe("荷兰");
    expect(detectRegion("澳洲雪梨")).toBe("澳大利亚");
    expect(detectRegion("馬來西亞")).toBe("马来西亚");
    expect(detectRegion("俄羅斯莫斯科")).toBe("俄罗斯");
  });

  it("Scenario 6: avoids false positives on generic Chinese phrases and falls back to 其他地区", () => {
    expect(detectRegion("自动选择")).toBe("其他地区");
    expect(detectRegion("故障转移")).toBe("其他地区");
    expect(detectRegion("IEPL 专线 01")).toBe("其他地区");
    expect(detectRegion("家宽原生 02")).toBe("其他地区");
    expect(detectRegion("移动/联通/电信 BGP")).toBe("其他地区");
    expect(detectRegion("高速加密节点")).toBe("其他地区");
    expect(detectRegion("完美负载均衡")).toBe("其他地区");

    // Single-char shorthand queries and numbered single-char nodes still work
    expect(detectRegion("港")).toBe("中国香港");
    expect(detectRegion("美")).toBe("美国");
    expect(detectRegion("日")).toBe("日本");
    expect(detectRegion("港01")).toBe("中国香港");
    expect(detectRegion("日-02")).toBe("日本");
    expect(detectRegion("美 01")).toBe("美国");
  });

  it("detectRegions strips confirmed cross-region brand prefix even when brand lacks flavor keywords", () => {
    const names = [
      "香港联通-美国01",
      "香港联通-日本01",
      "香港联通-新加坡01",
      "香港联通-01",
    ];
    expect(detectRegions(names)).toEqual([
      "美国",
      "日本",
      "新加坡",
      "中国香港",
    ]);
  });
});
