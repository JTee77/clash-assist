import { describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

import { openVergeWithFeedback, noticeHasRecheck, noticeView } from "./DiagnosticsNotices";
import { format } from "../i18n";
import { zhHans } from "../i18n/locales/zh-Hans";
import { zhHant } from "../i18n/locales/zh-Hant";
import { en } from "../i18n/locales/en";
import { sourceNoticesOf, type SourceNotice } from "../lib/mihomo";
import backendLabels from "../lib/mihomo/backendSourceLabels.fixture.json";

describe("openVergeWithFeedback（升级按钮失败要有反馈）", () => {
  it("成功：先清掉旧提示，不报失败", async () => {
    const setFailed = vi.fn();
    await openVergeWithFeedback(setFailed, () => Promise.resolve());
    expect(setFailed.mock.calls).toEqual([[false]]);
  });

  it("失败：不吞错误，标记失败让按钮旁出一句提示", async () => {
    const setFailed = vi.fn();
    await openVergeWithFeedback(setFailed, () => Promise.reject(new Error("打开浏览器失败")));
    expect(setFailed.mock.calls).toEqual([[false], [true]]);
  });
});

describe("noticeView：提示级别与文案", () => {
  it("已有提示的级别不变", () => {
    expect(noticeView({ kind: "insecure-dacl", pipes: [] }).level).toBe("block");
    expect(noticeView({ kind: "untrusted-pipe" }).level).toBe("block");
    expect(noticeView({ kind: "tcp-unverified" }).level).toBe("warn");
    expect(noticeView({ kind: "tcp-rejected", code: "token-mismatch" })).toEqual({
      level: "warn",
      message: "diag.tcpRejected.tokenMismatch",
    });
    expect(noticeView({ kind: "tcp-rejected", code: "x-y" })).toEqual({
      level: "warn",
      message: "diag.tcpRejected.other",
      vars: { code: "x-y" },
    });
  });

  it("pipe-open-to-all 是警告（Ellie：任何本机用户都能经该管道操作控制接口），不是拒绝", () => {
    expect(noticeView({ kind: "pipe-open-to-all", pipes: ["p"] })).toEqual({
      level: "warn",
      message: "diag.pipeOpenToAll",
    });
    expect(noticeView({ kind: "pipe-open-to-all", pipes: [] }).level).toBe("warn");
    expect(zhHans["diag.pipeOpenToAll"]).toBe(
      "Clash Verge 的控制管道对本机所有用户开放，其他用户可借此操控内核，目前上游尚无修复。",
    );
    // 不阻止连接：文案不能说已拒绝 / 已停止
    for (const m of [zhHans, zhHant, en]) {
      expect(m["diag.pipeOpenToAll"]).not.toMatch(/拒绝|拒絕|停止|refused|stopped/i);
    }
  });

  it("service-mode 是警告（取代 tcp-unverified），ipv6-dual-stack 是说明；端口唯一时带端口，否则用通用句", () => {
    expect(noticeView({ kind: "service-mode" })).toEqual({ level: "warn", message: "diag.serviceMode" });
    expect(noticeView({ kind: "ipv6-dual-stack", ports: [9097] })).toEqual({
      level: "info",
      message: "diag.ipv6DualStack",
      vars: { port: 9097 },
    });
    expect(noticeView({ kind: "ipv6-dual-stack", ports: [] }).message).toBe("diag.ipv6DualStackGeneric");
    expect(noticeView({ kind: "ipv6-dual-stack", ports: [1, 2] }).message).toBe("diag.ipv6DualStackGeneric");
    expect(format("zh-Hans", "diag.ipv6DualStack", { port: 9097 })).toBe(
      "控制端口 9097 只监听了 IPv6，请在 Clash Verge 设置中把外部控制器地址改为 127.0.0.1:9097 后重新检测。",
    );
  });

  it("新文案三种语言各一句话，占位符一致", () => {
    const keys = ["diag.pipeOpenToAll", "diag.serviceMode", "diag.ipv6DualStack", "diag.ipv6DualStackGeneric"] as const;
    for (const k of keys) {
      const vars = (zhHans[k].match(/\{\w+\}/g) ?? []).sort();
      for (const [lang, m] of [["zh-Hans", zhHans], ["zh-Hant", zhHant], ["en", en]] as const) {
        const s = m[k];
        expect(s.trim(), `${lang} ${k}`).toBe(s);
        expect((s.match(/\{\w+\}/g) ?? []).sort(), `${lang} ${k}`).toEqual(vars);
        // 一句话：只在结尾有一个句末标点
        const ends = s.replace(/127\.0\.0\.1(:\{port\})?/g, "").match(/[。！？.!?]/g) ?? [];
        expect(ends.length, `${lang} ${k}: ${s}`).toBe(1);
        expect(s, `${lang} ${k}`).toMatch(/[。.]$/);
      }
    }
  });

  it("后端样本的每条提示都有文案（不会落到缺失的键）", () => {
    for (const n of sourceNoticesOf(backendLabels.labels.map((s) => s.label))) {
      const v = noticeView(n);
      for (const lang of ["zh-Hans", "zh-Hant", "en"] as const) {
        const text = format(lang, v.message, v.vars);
        expect(text, `${lang} ${n.kind}`).not.toMatch(/\{\w+\}/);
        expect(text.length, `${lang} ${n.kind}`).toBeGreaterThan(0);
      }
    }
  });
});

describe("PR #56 文案", () => {
  it("service-mode 文案包含「无法验证监听方」及对应译文，并说明已拒绝连接", () => {
    expect(zhHans["diag.serviceMode"]).toContain("无法验证监听方");
    expect(zhHant["diag.serviceMode"]).toContain("無法驗證監聽方");
    expect(en["diag.serviceMode"]).toMatch(/listener can't be verified/);
    expect(zhHans["diag.serviceMode"]).toContain("已拒绝连接");
    expect(zhHant["diag.serviceMode"]).toContain("已拒絕連線");
    expect(en["diag.serviceMode"]).toContain("connection was refused");
  });
  it("IPv6 文案用「外部控制器」设置名，不写死点击路径", () => {
    expect(format("zh-Hans", "diag.ipv6DualStack", { port: 9097 })).toBe(
      "控制端口 9097 只监听了 IPv6，请在 Clash Verge 设置中把外部控制器地址改为 127.0.0.1:9097 后重新检测。",
    );
    for (const m of [zhHans, zhHant]) {
      expect(m["diag.ipv6DualStack"]).toContain("外部控制器");
      expect(m["diag.ipv6DualStackGeneric"]).toContain("外部控制器");
    }
    expect(en["diag.ipv6DualStack"]).toContain("external controller");
    expect(en["diag.ipv6DualStackGeneric"]).toContain("external controller");
    for (const m of [zhHans, zhHant, en]) {
      // 不写死点击路径（「设置 → 外部控制器」之类）；<原端口> 这样的字面占位不算
      expect(m["diag.ipv6DualStack"] + m["diag.ipv6DualStackGeneric"]).not.toMatch(/→|->|\s>\s/);
    }
  });
  it("无端口的 IPv6 通用句写成 127.0.0.1:<原端口>（S-3），不含未替换的变量", () => {
    expect(format("zh-Hans", "diag.ipv6DualStackGeneric")).toBe(
      "控制端口只监听了 IPv6，请在 Clash Verge 设置中把外部控制器地址改为 127.0.0.1:<原端口> 后重新检测。",
    );
    expect(zhHant["diag.ipv6DualStackGeneric"]).toContain("127.0.0.1:<原連接埠>");
    expect(en["diag.ipv6DualStackGeneric"]).toContain("127.0.0.1:<original port>");
  });
  it("pipe-open-to-all（S-2 / M-2）：一句话说明管道对所有用户开放、可被操控内核、上游尚无修复；不让用户重新检测，也不带重新检测按钮", () => {
    expect(zhHans["diag.pipeOpenToAll"]).toBe("Clash Verge 的控制管道对本机所有用户开放，其他用户可借此操控内核，目前上游尚无修复。");
    expect(zhHant["diag.pipeOpenToAll"]).toBe("Clash Verge 的控制管道對本機所有使用者開放，其他使用者可藉此操控核心，目前上游尚無修復。");
    expect(en["diag.pipeOpenToAll"]).toBe("Clash Verge's control pipe is open to all users on this PC, so other users can control the core through it; there is no upstream fix yet.");
    for (const m of [zhHans, zhHant, en]) {
      // M-2：升级不能消除问题，也不能淡化风险
      expect(m["diag.pipeOpenToAll"]).not.toMatch(/升级|升級|upgrade/i);
      expect(m["diag.pipeOpenToAll"]).not.toMatch(/不影响|不影響|doesn't affect|does not affect/i);
      expect(m["diag.pipeOpenToAll"]).not.toMatch(/重新检测|重新檢測|check again/i);
    }
    expect(noticeHasRecheck({ kind: "pipe-open-to-all", pipes: [] })).toBe(false);
    const others: SourceNotice[] = [
      { kind: "insecure-dacl", pipes: [] },
      { kind: "untrusted-pipe" },
      { kind: "tcp-unverified" },
      { kind: "service-mode" },
      { kind: "tcp-rejected", code: "token-mismatch" },
      { kind: "ipv6-dual-stack", ports: [9097] },
    ];
    for (const n of others) {
      expect(noticeHasRecheck(n), n.kind).toBe(true);
    }
  });
  it("截断文案已随 sourceItemsText 一起删除（S-1）", () => {
    for (const m of [zhHans, zhHant, en] as Record<string, string>[]) expect(m["diag.truncated"]).toBeUndefined();
  });
});
