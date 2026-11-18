/**
 * Windows FlClash 与 macOS 共用同一套控制器操作（刷新、当前节点、切换、切回）。
 * 传输差异在 Rust：Windows 读 `%APPDATA%\com.follow\clash`，经 TCP 连 FlClashCore.exe。
 * 这里不连真实 FlClash。
 */
import { beforeEach, describe, expect, it, vi } from "vitest";

const h = vi.hoisted(() => ({
  invoke: vi.fn(),
}));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: h.invoke,
}));

import { discoverAndProbe, getProxies, restoreProxy, switchProxy } from "./client";
import { flclashLocalMessage } from "./clients";
import type { ControllerConfig } from "./types";

const WIN_SOURCE =
  "flclash:C:\\Users\\ada\\AppData\\Roaming\\com.follow\\clash\\shared_preferences.json";

const windowsFlclash = (mode: string): ControllerConfig => ({
  host: "127.0.0.1",
  port: 9090,
  secret: "",
  mixedPort: 7890,
  source: WIN_SOURCE,
  sockPath: null,
  mode,
});

const HK = "日本 01";
const US = "美国 02";
const MAIN = "节点选择";

function proxiesBody(mainNow: string, globalNow: string): string {
  return JSON.stringify({
    proxies: {
      GLOBAL: { name: "GLOBAL", type: "Selector", now: globalNow, all: ["DIRECT", HK, US] },
      [MAIN]: { name: MAIN, type: "Selector", now: mainNow, all: [HK, US] },
      [HK]: { name: HK, type: "Vless" },
      [US]: { name: US, type: "Vless" },
      DIRECT: { name: "DIRECT", type: "Direct" },
    },
  });
}

describe("Windows FlClash 发现提示", () => {
  beforeEach(() => {
    h.invoke.mockReset();
    (globalThis as unknown as { window?: Record<string, unknown> }).window = {
      __TAURI_INTERNALS__: {},
    };
  });

  it("没有配置文件时提示安装并开启外部控制器", async () => {
    h.invoke.mockImplementation(async (cmd: string) => {
      if (cmd === "discover_mihomo_for_client") {
        return {
          host: "127.0.0.1",
          port: 9090,
          secret: "",
          mixedPort: 7890,
          source: "flclash:defaults",
          sockPath: null,
        };
      }
      throw new Error("控制器端口没有监听者");
    });
    const state = await discoverAndProbe(undefined, "flclash");
    expect(state.status).toBe("unreachable");
    expect(state.message).toContain("未检测到 FlClash");
    expect(state.message).not.toContain("已找到 FlClash");
  });

  it("外部控制器关闭、未运行、密钥不匹配、内核不符是不同提示", async () => {
    expect(flclashLocalMessage("flclash-ec-disabled:C:\\x\\shared_preferences.json")).toContain(
      "未开启外部控制器",
    );
    expect(flclashLocalMessage("flclash:defaults")).toContain("未检测到 FlClash");
    expect(flclashLocalMessage(WIN_SOURCE)).toContain("没有在监听");
    expect(flclashLocalMessage(WIN_SOURCE, "监听者校验失败：image-mismatch：FlClash.exe")).toContain(
      "FlClashCore",
    );
    expect(flclashLocalMessage("flclash-ec-remote:C:\\x\\shared_preferences.json")).toContain(
      "不是本机地址",
    );

    h.invoke.mockImplementation(async (cmd: string) => {
      if (cmd === "discover_mihomo_for_client") {
        return {
          host: "127.0.0.1",
          port: 9090,
          secret: "",
          mixedPort: 7890,
          source: WIN_SOURCE,
          sockPath: null,
        };
      }
      if (cmd === "mihomo_http") return { status: 401, body: "" };
      throw new Error(`unexpected ${cmd}`);
    });
    const denied = await discoverAndProbe(undefined, "flclash");
    expect(denied.status).toBe("unauthorized");
    expect(denied.message).toContain("密钥不匹配");
    expect(denied.config?.secret).toBe("");
    expect(denied.config?.sockPath).toBeNull();
  });

  it("配置在但控制器没听时，探测失败走未运行提示", async () => {
    h.invoke.mockImplementation(async (cmd: string) => {
      if (cmd === "discover_mihomo_for_client") {
        return {
          host: "127.0.0.1",
          port: 9090,
          secret: "",
          mixedPort: 7890,
          source: WIN_SOURCE,
          sockPath: null,
        };
      }
      throw new Error("控制器端口没有监听者");
    });
    const state = await discoverAndProbe(undefined, "flclash");
    expect(state.status).toBe("unreachable");
    expect(state.message).toContain("没有在监听");
    expect(state.message).not.toContain("未开启外部控制器");

    h.invoke.mockImplementation(async (cmd: string) => {
      if (cmd === "discover_mihomo_for_client") {
        return {
          host: "127.0.0.1",
          port: 9090,
          secret: "",
          mixedPort: 7890,
          source: WIN_SOURCE,
          sockPath: null,
        };
      }
      throw new Error("监听者校验失败：image-mismatch：FlClash.exe");
    });
    const wrong = await discoverAndProbe(undefined, "flclash");
    expect(wrong.message).toContain("FlClashCore");
  });
});

describe("Windows FlClash 节点、切换与切回", () => {
  beforeEach(() => {
    h.invoke.mockReset();
    (globalThis as unknown as { window?: Record<string, unknown> }).window = {
      __TAURI_INTERNALS__: {},
    };
  });

  it("刷新节点走 TCP、不走命名管道，并认出规则模式下的当前节点", async () => {
    h.invoke.mockImplementation(async (cmd: string, args: { req: { path?: string; sockPath?: string | null; secret?: string } }) => {
      if (cmd === "mihomo_http" && args.req.path === "/configs") {
        return { status: 200, body: JSON.stringify({ "mixed-port": 7890, mode: "rule" }) };
      }
      if (cmd === "mihomo_list_nodes") {
        expect(args.req.sockPath).toBeNull();
        expect(args.req.secret).toBe("");
        return {
          status: 200,
          unauthorized: false,
          error: null,
          transport: "tcp",
          nodes: [
            { name: HK, type: "Vless" },
            { name: US, type: "Vless" },
          ],
          groups: [
            { name: "GLOBAL", type: "Selector", now: US, all: ["DIRECT", HK, US] },
            { name: MAIN, type: "Selector", now: HK, all: [HK, US] },
          ],
        };
      }
      throw new Error(cmd);
    });
    const list = await getProxies(windowsFlclash("rule"));
    expect(list.unauthorized).toBe(false);
    expect(list.currentProxy).toBe(HK);
    expect(list.nodes.map((n) => n.name)).toEqual([HK, US]);
    expect(h.invoke.mock.calls.some(([cmd]) => cmd === "mihomo_unix_http")).toBe(false);
  });

  it("规则模式切到主选择组，全局模式切到 GLOBAL，直连模式不发请求", async () => {
    const puts: string[] = [];
    let reads = 0;
    h.invoke.mockImplementation(async (_cmd: string, args: { req: { method: string; path: string; body?: string | null } }) => {
      if (args.req.method === "GET") {
        reads += 1;
        return { status: 200, body: proxiesBody(reads === 1 ? HK : US, HK) };
      }
      puts.push(`${args.req.path} ${JSON.parse(args.req.body ?? "{}").name}`);
      return { status: 204, body: "" };
    });
    await expect(switchProxy(windowsFlclash("rule"), "GLOBAL", US)).resolves.toBe(true);
    expect(puts.some((p) => p.startsWith(`/proxies/${encodeURIComponent(MAIN)}`))).toBe(true);
    expect(puts.some((p) => p.startsWith("/proxies/GLOBAL"))).toBe(false);

    puts.length = 0;
    h.invoke.mockImplementation(async (_cmd: string, args: { req: { method: string; path: string; body?: string | null } }) => {
      if (args.req.method === "GET") {
        const switched = puts.length > 0;
        return { status: 200, body: proxiesBody(HK, switched ? US : HK) };
      }
      puts.push(args.req.path);
      return { status: 204, body: "" };
    });
    await expect(switchProxy(windowsFlclash("global"), MAIN, US)).resolves.toBe(true);
    expect(puts).toEqual(["/proxies/GLOBAL"]);

    h.invoke.mockClear();
    await expect(switchProxy(windowsFlclash("direct"), MAIN, US)).resolves.toBe(false);
    expect(h.invoke).not.toHaveBeenCalled();
  });

  it("切回把原先节点 PUT 回主选择组", async () => {
    const puts: Array<{ path: string; name: string }> = [];
    h.invoke.mockImplementation(async (_cmd: string, args: { req: { method: string; path: string; body?: string | null } }) => {
      expect(args.req.method).toBe("PUT");
      puts.push({ path: args.req.path, name: JSON.parse(args.req.body ?? "{}").name });
      return { status: 204, body: "" };
    });
    await expect(
      restoreProxy(windowsFlclash("rule"), { group: MAIN, now: HK }),
    ).resolves.toBe(true);
    expect(puts).toEqual([{ path: `/proxies/${encodeURIComponent(MAIN)}`, name: HK }]);
  });
});
