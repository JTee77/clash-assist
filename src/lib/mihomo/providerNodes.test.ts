import { describe, expect, it, vi, beforeEach } from "vitest";
import { mergeProviderNodes, missingProviderNames, resolveProviderNode } from "./providerNodes";
import type { ProxyInfo } from "./types";
const h = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: h.invoke }));
import { probeDelay, defaultConfig, getProxies } from "./client";

describe("provider nodes", () => {
  const groups: Record<string, ProxyInfo> = { Proxy: { name: "Proxy", type: "Selector", now: "only-sub", all: ["static", "only-sub"] }, static: { name: "static", type: "Trojan" } };
  it("supplements missing members without replacing existing proxies or exposing unused subscriptions", () => {
    const merged = mergeProviderNodes(groups, { file: { vehicleType: "File", proxies: [{ name: "only-sub", type: "Vless", udp: true }, { name: "unused", type: "Vless" }, { name: "static", type: "Vless" }] } });
    expect(missingProviderNames(merged)).toEqual([]);
    expect(merged["only-sub"].udp).toBe(true);
    expect(merged.static.type).toBe("Trojan");
    expect(merged.unused).toBeUndefined();
    expect(groups["only-sub"]).toBeUndefined();
  });
  it("prefers one real subscription over virtual compatible copies", () => {
    expect(resolveProviderNode({ group: { vehicleType: "Compatible", proxies: [{ name: "a", type: "Vless" }] }, subscription: { vehicleType: "HTTP", proxies: [{ name: "a", type: "Vless" }] } }, "a")?.provider).toBe("subscription");
  });
  it("rejects duplicate names across subscriptions and missing node metadata", () => {
    expect(resolveProviderNode({ one: { vehicleType: "File", proxies: [{ name: "a", type: "Vless" }] }, two: { vehicleType: "HTTP", proxies: [{ name: "a", type: "Vless" }] } }, "a")).toBeNull();
    expect(resolveProviderNode({ one: { proxies: [{ name: "a" } as ProxyInfo] } }, "a")).toBeNull();
  });
});

describe("provider-only delay probe", () => {
  beforeEach(() => {
    h.invoke.mockReset();
    vi.stubGlobal("fetch", vi.fn().mockRejectedValue(new Error("no real network in fixtures")));
    (globalThis as unknown as { window: Record<string, unknown> }).window = { __TAURI_INTERNALS__: {} };
  });
  it("uses the provider healthcheck route after leaf /proxies returns 404, with escaped names and GET only", async () => {
    h.invoke.mockImplementation(async (_: string, { req }: { req: { path: string; method: string } }) => {
      expect(req.method).toBe("GET");
      if (req.path.startsWith("/proxies/")) return { status: 404, body: "{}" };
      if (req.path === "/providers/proxies") return { status: 200, body: JSON.stringify({ providers: { "sub /日本": { vehicleType: "File", proxies: [{ name: "JP /1", type: "Vless" }] } } }) };
      expect(req.path).toContain("/providers/proxies/sub%20%2F%E6%97%A5%E6%9C%AC/JP%20%2F1/healthcheck?");
      return { status: 200, body: '{"delay":42}' };
    });
    expect(await probeDelay({ ...defaultConfig(), sockPath: null }, "JP /1", "https://example.com", 1500)).toBe(42);
    expect(h.invoke.mock.calls.map(([, args]) => args.req.path).filter((path: string) => path === "/providers/proxies")).toHaveLength(1);
  });
  it("does not pick an ambiguous provider or use provider fallback for authentication errors", async () => {
    h.invoke.mockResolvedValueOnce({ status: 404, body: "{}" }).mockResolvedValueOnce({ status: 200, body: JSON.stringify({ providers: { one: { proxies: [{ name: "a", type: "Vless" }] }, two: { proxies: [{ name: "a", type: "Vless" }] } } }) });
    expect(await probeDelay(defaultConfig(), "a", "https://example.com")).toBeNull();
    expect(h.invoke.mock.calls.some(([, args]) => args.req.path.includes("healthcheck"))).toBe(false);
    h.invoke.mockReset().mockResolvedValue({ status: 401, body: "{}" });
    expect(await probeDelay(defaultConfig(), "a", "https://example.com")).toBeNull();
    expect(h.invoke.mock.calls.every(([, args]) => args.req.path.startsWith("/proxies/"))).toBe(true);
  });
});

describe("provider reads stay on one controller", () => {
  beforeEach(() => {
    h.invoke.mockReset();
    (globalThis as unknown as { window: Record<string, unknown> }).window = { __TAURI_INTERNALS__: {} };
    vi.stubGlobal("fetch", vi.fn().mockRejectedValue(new Error("no real network")));
  });
  it("does not borrow a TCP healthcheck when the Unix provider endpoint is unavailable", async () => {
    h.invoke.mockImplementation(async (cmd: string) => {
      if (cmd === "mihomo_http") return { status: 200, body: '{"delay":99}' };
      return { status: 404, body: "{}" };
    });
    expect(await probeDelay({ ...defaultConfig(), sockPath: "/fixture/controller.sock" }, "a", "https://example.com")).toBeNull();
    expect(h.invoke.mock.calls.map(([cmd]) => cmd)).toEqual(["mihomo_unix_http", "mihomo_unix_http"]);
    expect(fetch).not.toHaveBeenCalled();
  });
  it("preserves partial static results and reports unavailable providers in the browser fallback", async () => {
    (globalThis as unknown as { window: Record<string, unknown> }).window = {};
    vi.stubGlobal("fetch", vi.fn().mockImplementation(async (url: string) => {
      if (url.endsWith("/proxies") && !url.includes("/providers/")) return new Response(JSON.stringify({ proxies: { Proxy: { name: "Proxy", type: "Selector", now: "only-sub", all: ["only-sub", "static"] }, static: { name: "static", type: "Trojan" } } }), { status: 200 });
      return new Response("{}", { status: 403 });
    }));
    const result = await getProxies({ ...defaultConfig(), sockPath: null });
    expect(result.nodes.map((n) => n.name)).toEqual(["static"]);
    expect(result.error).toContain("部分订阅节点");
    expect(result.currentProxy).toBeNull();
    expect(h.invoke).not.toHaveBeenCalled();
  });
});
