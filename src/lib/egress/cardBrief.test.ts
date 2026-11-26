import { describe, it, expect } from "vitest";
import type { CheckCard, CheckLevel } from "./types";
import { formatCardBrief } from "./cardBrief";

function card(
  id: string,
  level: CheckLevel,
  conclusion: string,
  metrics?: CheckCard["metrics"],
  extra?: Partial<CheckCard>,
): CheckCard {
  return { id, title: id, level, conclusion, metrics, ...extra };
}

describe("formatCardBrief", () => {
  it("exit-ip：国码 + 机房/家宽；失败无出口；IP 不进卡内", () => {
    expect(
      formatCardBrief(
        card("exit-ip", "pass", "1.2.3.4 · US · 住宅(ISP)", {
          exitIp: "1.2.3.4",
          countryCode: "us",
          hosting: false,
        }),
      ),
    ).toBe("US · 家宽");
    expect(
      formatCardBrief(
        card("exit-ip", "warn", "9.9.9.9 · JP · 疑似机房", {
          exitIp: "9.9.9.9",
          countryCode: "JP",
          hosting: true,
        }),
      ),
    ).toBe("JP · 机房");
    expect(
      formatCardBrief(
        card("exit-ip", "fail", "无法获取出口 IP", {
          exitIp: null,
          countryCode: null,
          hosting: null,
        }),
      ),
    ).toBe("无出口");
    expect(
      formatCardBrief(
        card("exit-ip", "pass", "1.2.3.4 · US · 住宅", {
          exitIp: "1.2.3.4",
          countryCode: "US",
          hosting: false,
        }),
      ),
    ).not.toContain("1.2.3.4");
    expect(
      formatCardBrief(
        card("exit-ip", "unknown", "23.106.130.20 · CA · 未判定", {
          exitIp: "23.106.130.20",
          countryCode: "CA",
          hosting: null,
        }),
      ),
    ).toBe("CA · 未判定");
    expect(
      formatCardBrief(
        card("exit-ip", "unknown", "96.45.186.222 · US · 未判定", {
          exitIp: "96.45.186.222",
          countryCode: "US",
          hosting: false,
        }),
      ),
    ).toBe("US · 未判定");
  });

  it("latency：N ms 或失败", () => {
    expect(
      formatCardBrief(
        card("latency", "pass", "大约 123 ms（3/3 次中位）", { latencyMs: 123.4 }),
      ),
    ).toBe("123 ms");
    expect(
      formatCardBrief(
        card("latency", "fail", "采样失败", { latencyMs: null }),
      ),
    ).toBe("失败");
  });

  it("bandwidth：↓x / ↑y 两行（有 metrics）", () => {
    expect(
      formatCardBrief(
        card("bandwidth", "pass", "文案可很长 ↓ 60 Mbps · ↑ 20 Mbps", {
          downMbps: 60,
          upMbps: 20,
        }),
      ),
    ).toBe("↓60\n↑20");
    expect(
      formatCardBrief(
        card("bandwidth", "warn", "偏慢", { downMbps: 2.5, upMbps: 1.25 }),
      ),
    ).toBe("↓2.5\n↑1.25");
    expect(
      formatCardBrief(
        card("bandwidth", "fail", "抽样失败", { downMbps: null, upMbps: null }),
      ),
    ).toBe("失败");
  });

  it("reachability：通/不通/部分通", () => {
    expect(formatCardBrief(card("reachability", "pass", "4/4 境外探测点能通（约 30 ms）"))).toBe(
      "通",
    );
    expect(formatCardBrief(card("reachability", "fail", "无法经代理访问"))).toBe("不通");
    expect(formatCardBrief(card("reachability", "warn", "2/4 能通"))).toBe("部分通");
    expect(
      formatCardBrief(
        card("reachability", "unknown", "未验证", undefined, { unverified: true }),
      ),
    ).toBe("未验证");
  });

  it("服务类：区码 / 开 / 锁；无 metrics 截短 conclusion", () => {
    expect(
      formatCardBrief(
        card("chatgpt", "pass", "可用（US）", {
          unlockSupported: true,
          unlockLevel: "full",
          unlockRegion: "US",
        }),
      ),
    ).toBe("US");
    expect(
      formatCardBrief(
        card("gemini", "fail", "不可用", {
          unlockSupported: false,
          unlockLevel: "blocked",
          unlockRegion: "CN",
        }),
      ),
    ).toBe("锁·CN");
    expect(
      formatCardBrief(
        card("netflix", "pass", "可用", { unlockSupported: true, unlockLevel: "pass" }),
      ),
    ).toBe("开");
    expect(
      formatCardBrief(
        card("disney", "fail", "不可用（地区限制）", {
          unlockSupported: false,
          unlockLevel: "fail",
        }),
      ),
    ).toBe("锁");
    expect(
      formatCardBrief(card("youtube", "pass", "可用，会员信号偏弱且文案很长很长")),
    ).toBe("可用，会员信号偏弱且文…");
  });

  it("服务汇总：全通 / 部分 / 全不通 / 检测中 / 未测", () => {
    expect(formatCardBrief(card("svc-streaming", "pass", "Netflix：通"))).toBe("全通");
    expect(formatCardBrief(card("svc-ai", "warn", "Gemini：通\nChatGPT：不通"))).toBe(
      "部分",
    );
    expect(formatCardBrief(card("svc-store", "fail", "App Store：不通"))).toBe("全不通");
    expect(
      formatCardBrief(
        card("app-store", "pass", "可用（US）", {
          unlockSupported: true,
          unlockRegion: "US",
        }),
      ),
    ).toBe("通");
    expect(
      formatCardBrief(
        card("google-play", "fail", "不可用（地区限制）", {
          unlockSupported: false,
          unlockRegion: "US",
        }),
      ),
    ).toBe("不通");
    expect(formatCardBrief(card("svc-streaming", "running", "检测中…"))).toBe("检测中");
    expect(formatCardBrief(card("svc-streaming", "unknown", "Netflix：未测；原因很长"))).toBe("未测");
  });

  it("running / 尚未检测", () => {
    expect(formatCardBrief(card("latency", "running", "检测中…"))).toBe("检测中");
    expect(formatCardBrief(card("exit-ip", "unknown", "尚未检测"))).toBe("—");
  });
});
