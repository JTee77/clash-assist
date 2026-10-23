import { describe, it, expect } from "vitest";
import {
  classifyIpv6Leak,
  classifyWebRtc,
  candidateScope,
  isMdnsOrLocalName,
  type Ipv6Probe,
  type WebRtcSignal,
} from "./leakMatrix";

const v6 = (over: Partial<Ipv6Probe>): Ipv6Probe => ({
  proxyConfigured: true,
  directV4: "1.1.1.1",
  proxiedV4: "2.2.2.2",
  directV6: null,
  proxiedV6: null,
  ...over,
});

describe("classifyIpv6Leak", () => {
  it("两侧都无 IPv6 → pass（无从泄漏）", () => {
    expect(classifyIpv6Leak(v6({})).level).toBe("pass");
  });

  it("未连代理口 → unknown（不误判）", () => {
    const r = classifyIpv6Leak(v6({ proxyConfigured: false, directV6: "2400::1" }));
    expect(r.level).toBe("unknown");
    expect(r.suggestion).toContain("获取节点");
  });

  it("直连有 v6、代理无 → fail（旁路签名）", () => {
    const r = classifyIpv6Leak(v6({ directV6: "2400:abcd::1", proxiedV6: null }));
    expect(r.level).toBe("fail");
    expect(r.conclusion).toContain("绕过代理");
  });

  it("仅代理有 v6、直连无 → pass", () => {
    const r = classifyIpv6Leak(v6({ directV6: null, proxiedV6: "2400:proxy::1" }));
    expect(r.level).toBe("pass");
  });

  // 关键：以前"直连与代理相同 IPv6"一律 warn，现在按 IPv4 交叉验证给确定结论
  it("v6 同址 + IPv4 两路同源（TUN 全局接管）→ pass", () => {
    const r = classifyIpv6Leak(
      v6({ directV4: "9.9.9.9", proxiedV4: "9.9.9.9", directV6: "cafe::1", proxiedV6: "cafe::1" }),
    );
    expect(r.level).toBe("pass");
    expect(r.conclusion).toContain("统一隧道接管");
  });

  it("v6 同址 + IPv4 经代理改变 → pass（v6 被统一出口接管）", () => {
    const r = classifyIpv6Leak(
      v6({ directV4: "1.1.1.1", proxiedV4: "2.2.2.2", directV6: "cafe::1", proxiedV6: "cafe::1" }),
    );
    expect(r.level).toBe("pass");
  });

  it("v6 两路不同地址 → fail（独立直连 v6 出口）", () => {
    const r = classifyIpv6Leak(
      v6({ directV6: "2400:real::1", proxiedV6: "2400:proxy::9" }),
    );
    expect(r.level).toBe("fail");
    expect(r.conclusion).toContain("不一致");
  });
});

const wr = (over: Partial<WebRtcSignal>): WebRtcSignal => ({
  apiAvailable: true,
  gatherFailed: false,
  candidates: [],
  proxyExitIps: [],
  ...over,
});

describe("classifyWebRtc", () => {
  it("无 RTCPeerConnection → unknown", () => {
    expect(classifyWebRtc(wr({ apiAvailable: false })).level).toBe("unknown");
  });

  it("收集抛错 → warn（失败≠无泄漏）", () => {
    expect(classifyWebRtc(wr({ gatherFailed: true })).level).toBe("warn");
  });

  it("候选为空 → unknown", () => {
    expect(classifyWebRtc(wr({})).level).toBe("unknown");
  });

  it("仅内网/本地候选 → pass", () => {
    const r = classifyWebRtc(
      wr({
        candidates: [
          { type: "host", address: "192.168.1.20", scope: "private" },
          { type: "host", address: "fe80::1", scope: "private" },
        ],
        proxyExitIps: ["2.2.2.2"],
      }),
    );
    expect(r.level).toBe("pass");
    expect(r.conclusion).toContain("内网");
  });

  it("公网 srflx == 代理出口 → pass（STUN 也走了代理）", () => {
    const r = classifyWebRtc(
      wr({
        candidates: [{ type: "srflx", address: "2.2.2.2", scope: "public" }],
        proxyExitIps: ["2.2.2.2"],
      }),
    );
    expect(r.level).toBe("pass");
  });

  it("公网候选 ≠ 代理出口 → fail（疑似真实 IP）", () => {
    const r = classifyWebRtc(
      wr({
        candidates: [{ type: "srflx", address: "8.8.8.8", scope: "public" }],
        proxyExitIps: ["2.2.2.2"],
      }),
    );
    expect(r.level).toBe("fail");
    expect(r.conclusion).toContain("暴露");
  });

  it("有公网候选但无出口可比对 → warn（诚实标警）", () => {
    const r = classifyWebRtc(
      wr({
        candidates: [{ type: "srflx", address: "8.8.8.8", scope: "public" }],
        proxyExitIps: [],
      }),
    );
    expect(r.level).toBe("warn");
  });

  // 归属地判定（v0.1.11）：候选 ≠ 出口不再必然 fail
  it("公网候选 ≠ 出口，但归属(DE) ≠ 真实归属(CN) → pass（供应商基础设施）", () => {
    const r = classifyWebRtc(
      wr({
        candidates: [{ type: "srflx", address: "2a0c:59c0::1", scope: "public" }],
        proxyExitIps: ["157.119.102.175"],
        geoCountryByAddr: { "2a0c:59c0::1": "DE" },
        realCountry: "CN",
      }),
    );
    expect(r.level).toBe("pass");
    expect(r.conclusion).toContain("供应商基础设施");
  });

  it("公网候选归属 == 真实归属(CN) → fail（真泄漏）", () => {
    const r = classifyWebRtc(
      wr({
        candidates: [{ type: "srflx", address: "240e::1", scope: "public" }],
        proxyExitIps: ["157.119.102.175"],
        geoCountryByAddr: { "240e::1": "CN" },
        realCountry: "CN",
      }),
    );
    expect(r.level).toBe("fail");
  });

  it("归属未知 → 维持旧口径 fail（不放过疑似）", () => {
    const r = classifyWebRtc(
      wr({
        candidates: [{ type: "srflx", address: "8.8.8.8", scope: "public" }],
        proxyExitIps: ["2.2.2.2"],
        geoCountryByAddr: { "8.8.8.8": null },
        realCountry: "CN",
      }),
    );
    expect(r.level).toBe("fail");
  });

  it("混合：部分候选属供应商、部分疑似真实 → fail（只列疑似项）", () => {
    const r = classifyWebRtc(
      wr({
        candidates: [
          { type: "srflx", address: "2a0c:59c0::1", scope: "public" },
          { type: "srflx", address: "240e::9", scope: "public" },
        ],
        proxyExitIps: ["157.119.102.175"],
        geoCountryByAddr: { "2a0c:59c0::1": "DE", "240e::9": "CN" },
        realCountry: "CN",
      }),
    );
    expect(r.level).toBe("fail");
    expect(r.conclusion).toContain("240e::9");
    expect(r.conclusion).toContain("供应商基础设施");
  });
});

describe("candidateScope", () => {
  it("mDNS .local 归为 private（修复：曾被误判 public）", () => {
    expect(candidateScope("f1495443-f449-4cc1-b1e1-0e4da53e6171.local")).toBe("private");
    expect(isMdnsOrLocalName("anything.local")).toBe(true);
  });

  it("裸主机名（非 IP 字面量）归为 private", () => {
    expect(candidateScope("myhost")).toBe("private");
  });

  it("私网/回环/链路本地/ULA → private", () => {
    expect(candidateScope("192.168.1.20")).toBe("private");
    expect(candidateScope("10.0.0.5")).toBe("private");
    expect(candidateScope("172.16.0.1")).toBe("private");
    expect(candidateScope("127.0.0.1")).toBe("private");
    expect(candidateScope("::1")).toBe("private");
    expect(candidateScope("fe80::1")).toBe("private");
    expect(candidateScope("fd12::34")).toBe("private");
  });

  it("TUN fake-ip / CGNAT / IPv4-mapped → private（修复：曾被误判 public）", () => {
    // mihomo/Clash TUN 默认 fake-ip 段 198.18.0.0/15（RFC 2544 基准段，永不公网路由）
    expect(candidateScope("198.18.0.1")).toBe("private");
    expect(candidateScope("198.19.255.254")).toBe("private");
    // CGNAT 100.64.0.0/10（运营商 NAT / Tailscale 类网段）
    expect(candidateScope("100.64.0.1")).toBe("private");
    expect(candidateScope("100.127.255.254")).toBe("private");
    // IPv4-mapped IPv6 按映射的 v4 判定
    expect(candidateScope("::ffff:192.168.1.20")).toBe("private");
    expect(candidateScope("::ffff:198.18.0.1")).toBe("private");
  });

  it("段边界外仍是 public（不为让用例通过而扩段）", () => {
    expect(candidateScope("100.128.0.1")).toBe("public"); // CGNAT 上界外
    expect(candidateScope("100.63.255.254")).toBe("public"); // CGNAT 下界外
    expect(candidateScope("198.20.0.1")).toBe("public"); // fake-ip 段外
    expect(candidateScope("::ffff:8.8.8.8")).toBe("public"); // 映射的是公网 v4
  });

  it("可路由公网 IPv4/IPv6 → public", () => {
    expect(candidateScope("8.8.8.8")).toBe("public");
    expect(candidateScope("203.0.113.9")).toBe("public");
    expect(candidateScope("2400:abcd::1")).toBe("public");
  });
});

describe("classifyWebRtc + candidateScope 联动", () => {
  it("仅 .local 与私网候选 → pass（不再把隐私地址当公网暴露）", () => {
    // 用 candidateScope 真实归类构造候选，复现修复后的诊断链路
    const addrs = [
      "f1495443-f449-4cc1-b1e1-0e4da53e6171.local",
      "192.168.1.20",
    ];
    const r = classifyWebRtc({
      apiAvailable: true,
      gatherFailed: false,
      proxyExitIps: ["2.2.2.2"],
      candidates: addrs.map((a) => ({
        type: "host" as const,
        address: a,
        scope: candidateScope(a),
      })),
    });
    expect(r.level).toBe("pass");
  });

  it("TUN fake-ip host 候选 → pass（不再误报公网暴露）", () => {
    // TUN 开启时 ICE host 候选会枚举到 TUN 网卡地址 198.18.x.x
    const r = classifyWebRtc({
      apiAvailable: true,
      gatherFailed: false,
      proxyExitIps: ["2.2.2.2"],
      candidates: [
        { type: "host" as const, address: "198.18.0.1", scope: candidateScope("198.18.0.1") },
      ],
    });
    expect(r.level).toBe("pass");
  });

  it(".local + 真实公网 IP 混合 → 仍按公网 IP 判 fail", () => {
    const addrs = ["a1b2c3d4.local", "146.19.163.10"];
    const r = classifyWebRtc({
      apiAvailable: true,
      gatherFailed: false,
      proxyExitIps: ["2.2.2.2"],
      candidates: addrs.map((a) => ({
        type: "srflx" as const,
        address: a,
        scope: candidateScope(a),
      })),
    });
    expect(r.level).toBe("fail");
    // 暴露列表不应包含 .local
    expect(r.conclusion).toContain("146.19.163.10");
    expect(r.conclusion).not.toContain(".local");
  });
});
