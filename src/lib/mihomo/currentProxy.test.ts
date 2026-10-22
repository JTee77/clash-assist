import { describe, expect, it } from "vitest";
import {
  resolveCurrentProxy,
  resolveSelectorSnapshotFromMap,
  resolveSwitchPlanFromMap,
} from "./currentProxy";
import type { ProxyInfo } from "./types";

const leaf = (name: string, type: string): ProxyInfo => ({ name, type });
const group = (
  name: string,
  now: string,
  all: string[],
  type = "Selector",
): ProxyInfo => ({
  name,
  type,
  now,
  all,
});

function withLeaves(
  proxies: Record<string, ProxyInfo>,
  names: string[],
  type = "Vmess",
): Record<string, ProxyInfo> {
  for (const name of names) proxies[name] = leaf(name, type);
  return proxies;
}

describe("resolveCurrentProxy", () => {
  it("规则模式：GLOBAL 停在 DIRECT 时改取「节点选择」真实叶子", () => {
    const proxies: Record<string, ProxyInfo> = {
      GLOBAL: group("GLOBAL", "DIRECT", ["DIRECT", "HK17"]),
      节点选择: group("节点选择", "HK17", ["HK17", "SG2"]),
      NETFLIX: group("NETFLIX", "SG2", ["HK17", "SG2"]),
      HK17: leaf("HK17", "Hysteria2"),
      SG2: leaf("SG2", "Vmess"),
      DIRECT: leaf("DIRECT", "Direct"),
      REJECT: leaf("REJECT", "Reject"),
    };
    expect(resolveCurrentProxy(proxies)).toBe("HK17");
  });

  it("全局模式：GLOBAL 指向真实叶子时优先取它", () => {
    const proxies: Record<string, ProxyInfo> = {
      GLOBAL: group("GLOBAL", "jp-2", ["jp-2", "hk-1"]),
      节点选择: group("节点选择", "hk-1", ["jp-2", "hk-1"]),
      "jp-2": leaf("jp-2", "Shadowsocks"),
      "hk-1": leaf("hk-1", "Vmess"),
      DIRECT: leaf("DIRECT", "Direct"),
    };
    expect(resolveCurrentProxy(proxies)).toBe("jp-2");
  });

  it("无主组名且多子组并列 → null（诚实显示 —，不瞎猜）", () => {
    const proxies: Record<string, ProxyInfo> = {
      人工智能: group("人工智能", "us-1", ["us-1"]),
      NETFLIX: group("NETFLIX", "sg-9", ["sg-9"]),
      "us-1": leaf("us-1", "Vmess"),
      "sg-9": leaf("sg-9", "Trojan"),
      DIRECT: leaf("DIRECT", "Direct"),
    };
    expect(resolveCurrentProxy(proxies)).toBeNull();
  });

  it("纯直连模式（只有 GLOBAL→DIRECT）→ null", () => {
    const proxies: Record<string, ProxyInfo> = {
      GLOBAL: group("GLOBAL", "DIRECT", ["DIRECT"]),
      DIRECT: leaf("DIRECT", "Direct"),
    };
    expect(resolveCurrentProxy(proxies)).toBeNull();
  });

  it("唯一非主组且其 now 为真实叶子 → 采纳该叶子", () => {
    const proxies: Record<string, ProxyInfo> = {
      小鸡: group("小鸡", "hk-9", ["hk-9"]),
      "hk-9": leaf("hk-9", "Hysteria2"),
      DIRECT: leaf("DIRECT", "Direct"),
    };
    expect(resolveCurrentProxy(proxies)).toBe("hk-9");
  });

  it("组 now 指向另一个组（嵌套）时跟到真实叶子", () => {
    const proxies: Record<string, ProxyInfo> = {
      Proxy: group("Proxy", "内层", ["内层"]),
      内层: group("内层", "hk-1", ["hk-1"]),
      "hk-1": leaf("hk-1", "Vmess"),
      DIRECT: leaf("DIRECT", "Direct"),
    };
    // Proxy.now=内层 is a Selector; follow the chain to hk-1, never return the group.
    expect(resolveCurrentProxy(proxies)).toBe("hk-1");
  });

  it("Proxy 嵌套两层，旁边策略组指向别处也不抢", () => {
    const proxies: Record<string, ProxyInfo> = {
      Proxy: group("Proxy", "内层", ["内层"]),
      内层: group("内层", "再内层", ["再内层"]),
      再内层: group("再内层", "hk-1", ["hk-1"]),
      NETFLIX: group("NETFLIX", "sg-9", ["sg-9", "hk-1"]),
      AI: group("AI", "us-1", ["us-1"]),
      "hk-1": leaf("hk-1", "Vmess"),
      "sg-9": leaf("sg-9", "Trojan"),
      "us-1": leaf("us-1", "Vmess"),
      DIRECT: leaf("DIRECT", "Direct"),
    };
    expect(resolveCurrentProxy(proxies)).toBe("hk-1");
  });

  it("换订阅后主组是自定义名，取真实叶子最多的选择组", () => {
    const nodes = ["n0", "n1", "n2", "n3", "n4", "n5", "n6", "n7"];
    const proxies: Record<string, ProxyInfo> = {
      GLOBAL: group("GLOBAL", "DIRECT", ["DIRECT", ...nodes]),
      机场: group("机场", "n3", ["DIRECT", ...nodes]),
      NETFLIX: group("NETFLIX", "n1", ["n1", "n2"]),
      AI: group("AI", "n2", ["n2"]),
      Telegram: group("Telegram", "n0", ["n0", "n4"]),
      DIRECT: leaf("DIRECT", "Direct"),
    };
    withLeaves(proxies, nodes);
    expect(resolveCurrentProxy(proxies)).toBe("n3");
  });

  it("「🚀 节点选择」带表情前缀，策略组指向别的节点时仍取主组", () => {
    const main = ["HK-1", "JP-1", "US-1", "SG-1", "TW-1"];
    const proxies: Record<string, ProxyInfo> = {
      GLOBAL: group("GLOBAL", "DIRECT", ["DIRECT", ...main]),
      "🚀 节点选择": group("🚀 节点选择", "HK-1", [
        "♻️ 自动选择",
        ...main,
        "DIRECT",
      ]),
      "♻️ 自动选择": group("♻️ 自动选择", "JP-1", main, "URLTest"),
      NETFLIX: group("NETFLIX", "US-1", ["US-1", "SG-1"]),
      "🤖 AI": group("🤖 AI", "JP-1", ["JP-1"]),
      Telegram: group("Telegram", "SG-1", ["SG-1", "HK-1"]),
      DIRECT: leaf("DIRECT", "Direct"),
    };
    withLeaves(proxies, main, "Hysteria2");
    expect(resolveCurrentProxy(proxies)).toBe("HK-1");
  });

  it("主组只挂地区子组时跟着它的 now 走到叶子，不拿叶子更多的地区组", () => {
    const proxies: Record<string, ProxyInfo> = {
      GLOBAL: group("GLOBAL", "DIRECT", ["DIRECT"]),
      "🚀 节点选择": group("🚀 节点选择", "🇯🇵 日本节点", [
        "🇭🇰 香港节点",
        "🇯🇵 日本节点",
        "♻️ 自动选择",
      ]),
      "🇭🇰 香港节点": group("🇭🇰 香港节点", "hk-1", ["hk-1", "hk-2", "hk-3"]),
      "🇯🇵 日本节点": group("🇯🇵 日本节点", "jp-9", ["jp-9", "jp-8"]),
      "♻️ 自动选择": group("♻️ 自动选择", "hk-2", ["hk-1", "hk-2", "jp-9"], "URLTest"),
      NETFLIX: group("NETFLIX", "hk-1", ["hk-1", "jp-9"]),
      "hk-1": leaf("hk-1", "Vmess"),
      "hk-2": leaf("hk-2", "Vmess"),
      "hk-3": leaf("hk-3", "Vmess"),
      "jp-9": leaf("jp-9", "Vmess"),
      "jp-8": leaf("jp-8", "Vmess"),
      DIRECT: leaf("DIRECT", "Direct"),
    };
    expect(resolveCurrentProxy(proxies)).toBe("jp-9");
  });

  it("主组停在 DIRECT 时不拿策略组的节点充数", () => {
    const proxies: Record<string, ProxyInfo> = {
      GLOBAL: group("GLOBAL", "DIRECT", ["DIRECT"]),
      "🚀 节点选择": group("🚀 节点选择", "DIRECT", ["DIRECT", "HK-1", "JP-1", "US-1"]),
      NETFLIX: group("NETFLIX", "HK-1", ["HK-1"]),
      AI: group("AI", "JP-1", ["JP-1"]),
      Telegram: group("Telegram", "US-1", ["US-1"]),
      "HK-1": leaf("HK-1", "Vmess"),
      "JP-1": leaf("JP-1", "Vmess"),
      "US-1": leaf("US-1", "Vmess"),
      DIRECT: leaf("DIRECT", "Direct"),
    };
    expect(resolveCurrentProxy(proxies)).toBeNull();
  });

  it("两个一样大的自定义组且 now 不同 → null", () => {
    const proxies: Record<string, ProxyInfo> = {
      甲: group("甲", "a1", ["a1", "a2", "a3"]),
      乙: group("乙", "b1", ["b1", "b2", "b3"]),
      a1: leaf("a1", "Vmess"),
      a2: leaf("a2", "Vmess"),
      a3: leaf("a3", "Vmess"),
      b1: leaf("b1", "Vmess"),
      b2: leaf("b2", "Vmess"),
      b3: leaf("b3", "Vmess"),
      DIRECT: leaf("DIRECT", "Direct"),
    };
    expect(resolveCurrentProxy(proxies)).toBeNull();
  });

  it("不把 REJECT 或组名当成当前节点", () => {
    const proxies: Record<string, ProxyInfo> = {
      Proxy: group("Proxy", "内层", ["内层"]),
      内层: group("内层", "REJECT", ["REJECT"]),
      NETFLIX: group("NETFLIX", "内层", ["内层"]),
      REJECT: leaf("REJECT", "Reject"),
      DIRECT: leaf("DIRECT", "Direct"),
    };
    expect(resolveCurrentProxy(proxies)).toBeNull();
  });
});

describe("resolveSelectorSnapshotFromMap & resolveSwitchPlanFromMap", () => {
  it("规则模式下 GLOBAL=DIRECT 且带 Emoji 主组「🔰 节点选择」时，绝不误选 GLOBAL", () => {
    const nodes = ["香港高速3-HK21", "香港高速3-美国02 美区奈飞解锁", "香港高速3-美国82 奈飞"];
    const proxies: Record<string, ProxyInfo> = {
      GLOBAL: group("GLOBAL", "DIRECT", ["DIRECT", "REJECT", ...nodes]),
      "🔰 节点选择": group("🔰 节点选择", "香港高速3-HK21", [
        "♻️ 自动选择",
        "🎯 全球直连",
        ...nodes,
      ]),
      "♻️ 自动选择": group("♻️ 自动选择", "香港高速3-HK21", nodes, "URLTest"),
      "🎥 NETFLIX": group("🎥 NETFLIX", "🔰 节点选择", ["🔰 节点选择", ...nodes]),
      DIRECT: leaf("DIRECT", "Direct"),
      REJECT: leaf("REJECT", "Reject"),
    };
    withLeaves(proxies, nodes, "Vless");

    const snap = resolveSelectorSnapshotFromMap(proxies, "香港高速3-HK21");
    expect(snap).toEqual({
      group: "🔰 节点选择",
      now: "香港高速3-HK21",
    });

    const targetSnap = resolveSelectorSnapshotFromMap(
      proxies,
      "香港高速3-美国02 美区奈飞解锁",
    );
    expect(targetSnap?.group).toBe("🔰 节点选择");
  });

  it("当主组「🔰 节点选择」当前指向「♻️ 自动选择」(URLTest) 时，快照锁住外层 Selector 以便测完切回「♻️ 自动选择」", () => {
    const nodes = ["HK-1", "US-1"];
    const proxies: Record<string, ProxyInfo> = {
      GLOBAL: group("GLOBAL", "DIRECT", ["DIRECT", ...nodes]),
      "🔰 节点选择": group("🔰 节点选择", "♻️ 自动选择", ["♻️ 自动选择", ...nodes]),
      "♻️ 自动选择": group("♻️ 自动选择", "HK-1", nodes, "URLTest"),
      DIRECT: leaf("DIRECT", "Direct"),
    };
    withLeaves(proxies, nodes);

    const snap = resolveSelectorSnapshotFromMap(proxies, "HK-1");
    expect(snap?.group).toBe("🔰 节点选择");
    expect(snap?.now).toBe("♻️ 自动选择");
  });

  it("当主组只挂载地区子组（「🚀 节点选择」->「🇺🇸 美国节点」-> 节点）时，生成两步切换计划并记录完整恢复链", () => {
    const proxies: Record<string, ProxyInfo> = {
      GLOBAL: group("GLOBAL", "DIRECT", ["DIRECT"]),
      "🚀 节点选择": group("🚀 节点选择", "🇭🇰 香港节点", [
        "🇭🇰 香港节点",
        "🇺🇸 美国节点",
      ]),
      "🇭🇰 香港节点": group("🇭🇰 香港节点", "HK-1", ["HK-1", "HK-2"]),
      "🇺🇸 美国节点": group("🇺🇸 美国节点", "US-1", ["US-1", "US-2"]),
      "HK-1": leaf("HK-1", "Vmess"),
      "HK-2": leaf("HK-2", "Vmess"),
      "US-1": leaf("US-1", "Vmess"),
      "US-2": leaf("US-2", "Vmess"),
      DIRECT: leaf("DIRECT", "Direct"),
    };

    const snap = resolveSelectorSnapshotFromMap(proxies, "HK-1");
    expect(snap).toEqual({
      group: "🚀 节点选择",
      now: "🇭🇰 香港节点",
      restoreChain: [
        { group: "🚀 节点选择", now: "🇭🇰 香港节点" },
        { group: "🇭🇰 香港节点", now: "HK-1" },
        { group: "🇺🇸 美国节点", now: "US-1" },
      ],
    });

    const steps = resolveSwitchPlanFromMap(proxies, "US-2", "🚀 节点选择");
    expect(steps).toEqual([
      { group: "🇺🇸 美国节点", name: "US-2" },
      { group: "🚀 节点选择", name: "🇺🇸 美国节点" },
    ]);
  });

  it("自定义主组名（如「🐱 猫猫云」）无关键字时，按最大真实节点数准确命中主组而非 GLOBAL", () => {
    const nodes = ["N1", "N2", "N3", "N4"];
    const proxies: Record<string, ProxyInfo> = {
      GLOBAL: group("GLOBAL", "DIRECT", ["DIRECT", ...nodes]),
      "🐱 猫猫云": group("🐱 猫猫云", "N1", ["DIRECT", ...nodes]),
      AI: group("AI", "N2", ["N2"]),
      DIRECT: leaf("DIRECT", "Direct"),
    };
    withLeaves(proxies, nodes);

    const snap = resolveSelectorSnapshotFromMap(proxies, "N1");
    expect(snap).toEqual({
      group: "🐱 猫猫云",
      now: "N1",
      restoreChain: [
        { group: "🐱 猫猫云", now: "N1" },
        { group: "AI", now: "N2" },
      ],
    });
  });
});
