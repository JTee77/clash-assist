import { beforeEach, describe, expect, it, vi } from "vitest";

const h = vi.hoisted(() => ({
  invoke: vi.fn(),
}));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: h.invoke,
}));

import { switchProxy } from "./client";
import type { ControllerConfig } from "./types";

const HK21 = "香港高速3-HK21";
const HK17 = "香港高速3-HK17";
const US82 = "香港高速3-美国82 gemini";
const MAIN = "😣 节点选择";

const config = (mode: string): ControllerConfig => ({
  host: "127.0.0.1",
  port: 9097,
  secret: "s",
  mixedPort: 7897,
  source: "test",
  sockPath: null,
  mode,
});

function proxiesBody(mainNow: string, globalNow: string): string {
  return JSON.stringify({
    proxies: {
      GLOBAL: {
        name: "GLOBAL",
        type: "Selector",
        now: globalNow,
        all: ["DIRECT", HK21, HK17, US82],
      },
      [MAIN]: {
        name: MAIN,
        type: "Selector",
        now: mainNow,
        all: [US82, HK17, HK21],
      },
      [HK21]: { name: HK21, type: "Hysteria2" },
      [HK17]: { name: HK17, type: "Hysteria2" },
      [US82]: { name: US82, type: "Hysteria2" },
      DIRECT: { name: "DIRECT", type: "Direct" },
    },
  });
}

describe("switchProxy 按模式切流量组，并在测之前核对有效叶子", () => {
  beforeEach(() => {
    h.invoke.mockReset();
    (globalThis as unknown as { window?: Record<string, unknown> }).window = {
      __TAURI_INTERNALS__: {},
    };
  });

  it("规则模式：PUT 打在 😣 节点选择 上，复核叶子是目标才算切换成功", async () => {
    let reads = 0;
    h.invoke.mockImplementation(async (cmd: string, args: { req: { method: string; path: string; body?: string | null } }) => {
      expect(cmd).toBe("mihomo_http");
      const { method, path } = args.req;
      if (method === "GET" && path === "/proxies") {
        reads += 1;
        const mainNow = reads === 1 ? HK17 : US82;
        return { status: 200, body: proxiesBody(mainNow, HK21) };
      }
      if (method === "PUT") {
        expect(path).toBe(`/proxies/${encodeURIComponent(MAIN)}`);
        expect(JSON.parse(args.req.body ?? "{}").name).toBe(US82);
        return { status: 204, body: "" };
      }
      throw new Error(`${method} ${path}`);
    });

    await expect(switchProxy(config("rule"), "GLOBAL", US82)).resolves.toBe(true);
    const puts = h.invoke.mock.calls.filter(([, args]) => args.req.method === "PUT");
    expect(puts).toHaveLength(1);
    expect(puts.some(([, args]) => args.req.path === "/proxies/GLOBAL")).toBe(false);
  });

  it("PUT 成功但流量组有效叶子仍不是目标 → 切换失败（不能靠出口 IP 相同来放行）", async () => {
    h.invoke.mockImplementation(async (_cmd: string, args: { req: { method: string; path: string } }) => {
      if (args.req.method === "GET") {
        // GLOBAL 被写成 US82，节点选择仍停在 HK17：规则模式流量没过去。
        return { status: 200, body: proxiesBody(HK17, US82) };
      }
      return { status: 204, body: "" };
    });

    await expect(switchProxy(config("rule"), MAIN, US82)).resolves.toBe(false);
  });

  it("直连模式不发切换请求", async () => {
    await expect(switchProxy(config("direct"), MAIN, US82)).resolves.toBe(false);
    expect(h.invoke).not.toHaveBeenCalled();
  });

  it("全局模式切换并复核的是 GLOBAL", async () => {
    let reads = 0;
    h.invoke.mockImplementation(async (_cmd: string, args: { req: { method: string; path: string; body?: string | null } }) => {
      if (args.req.method === "GET") {
        reads += 1;
        const globalNow = reads === 1 ? HK21 : US82;
        return { status: 200, body: proxiesBody(HK17, globalNow) };
      }
      expect(args.req.path).toBe("/proxies/GLOBAL");
      expect(JSON.parse(args.req.body ?? "{}").name).toBe(US82);
      return { status: 204, body: "" };
    });

    await expect(switchProxy(config("global"), MAIN, US82)).resolves.toBe(true);
  });

  it("规则模式：节点选择不含目标时改切含该节点的 PROXY", async () => {
    const SS = "日本 SS";
    const HTTP = "新加坡 HTTP";
    let proxyNow = "DIRECT";
    h.invoke.mockImplementation(async (_cmd: string, args: { req: { method: string; path: string; body?: string | null } }) => {
      const { method, path } = args.req;
      if (method === "GET" && path === "/proxies") {
        return {
          status: 200,
          body: JSON.stringify({
            proxies: {
              GLOBAL: { name: "GLOBAL", type: "Selector", now: "DIRECT", all: ["DIRECT", SS] },
              节点选择: { name: "节点选择", type: "Selector", now: SS, all: [SS, "DIRECT"] },
              PROXY: { name: "PROXY", type: "Selector", now: proxyNow, all: [HTTP, "DIRECT"] },
              [SS]: { name: SS, type: "Shadowsocks" },
              [HTTP]: { name: HTTP, type: "Http" },
              DIRECT: { name: "DIRECT", type: "Direct" },
            },
          }),
        };
      }
      expect(method).toBe("PUT");
      expect(path).toBe("/proxies/PROXY");
      expect(JSON.parse(args.req.body ?? "{}").name).toBe(HTTP);
      proxyNow = HTTP;
      return { status: 204, body: "" };
    });

    await expect(switchProxy(config("rule"), "节点选择", HTTP)).resolves.toBe(true);
    const puts = h.invoke.mock.calls.filter(([, args]) => args.req.method === "PUT");
    expect(puts).toHaveLength(1);
  });

  it("订阅里的 Http 叶子不在原始 /proxies 中，合并后切换校验仍通过", async () => {
    const HTTP = "订阅新加坡 HTTP";
    let proxyNow = "DIRECT";
    h.invoke.mockImplementation(async (_cmd: string, args: { req: { method: string; path: string; body?: string | null } }) => {
      const { method, path } = args.req;
      if (method === "GET" && path === "/proxies") {
        return {
          status: 200,
          body: JSON.stringify({
            proxies: {
              GLOBAL: { name: "GLOBAL", type: "Selector", now: "DIRECT", all: ["DIRECT"] },
              PROXY: { name: "PROXY", type: "Selector", now: proxyNow, all: [HTTP, "DIRECT"] },
              DIRECT: { name: "DIRECT", type: "Direct" },
            },
          }),
        };
      }
      if (method === "GET" && path === "/providers/proxies") {
        return {
          status: 200,
          body: JSON.stringify({
            providers: {
              sub: { vehicleType: "File", proxies: [{ name: HTTP, type: "Http" }] },
            },
          }),
        };
      }
      expect(method).toBe("PUT");
      expect(path).toBe("/proxies/PROXY");
      expect(JSON.parse(args.req.body ?? "{}").name).toBe(HTTP);
      proxyNow = HTTP;
      return { status: 204, body: "" };
    });

    await expect(switchProxy(config("rule"), "PROXY", HTTP)).resolves.toBe(true);
  });
});
