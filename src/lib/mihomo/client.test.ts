import { describe, expect, it, vi, beforeEach } from "vitest";

const h = vi.hoisted(() => ({
  invoke: vi.fn(),
}));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: h.invoke,
}));

import {
  CLIENT_OPTIONS,
  clientUnreachableHint,
  discoverAndProbe,
  normalizeClientId,
} from "./index";

describe("0.2.3 FlClash client & live mixed-port sync", () => {
  beforeEach(() => {
    h.invoke.mockReset();
    (globalThis as unknown as { window?: Record<string, unknown> }).window = {
      __TAURI_INTERNALS__: {},
    };
  });

  it("normalizeClientId accepts verge and flclash, rejects others", () => {
    expect(normalizeClientId("verge")).toBe("verge");
    expect(normalizeClientId("flclash")).toBe("flclash");
    expect(normalizeClientId("mihomo_party")).toBeNull();
    expect(normalizeClientId("unknown")).toBeNull();
    const fl = CLIENT_OPTIONS.find((o) => o.id === "flclash");
    expect(fl?.disabled).toBeFalsy();
    expect(fl?.label).toBe("FlClash");
  });

  it("clientUnreachableHint differentiates flclash ec-disabled vs general", () => {
    expect(
      clientUnreachableHint("flclash", "flclash-ec-disabled:/tmp/com.follow.clash.plist"),
    ).toContain("检测到 FlClash 未开启外部控制器");
    expect(clientUnreachableHint("flclash", "flclash:defaults")).toContain(
      "设置 → 常规 → 入站",
    );
    expect(clientUnreachableHint("verge")).toContain("Clash Verge");
  });

  it("discoverAndProbe syncs live mixed-port from GET /configs when connected", async () => {
    h.invoke.mockImplementation(async (cmd: string, args?: { req?: { path?: string } }) => {
      if (cmd === "discover_mihomo_for_client") {
        return {
          host: "127.0.0.1",
          port: 9090,
          secret: "fl-32-secret",
          mixedPort: 7890, // stale static mixed-port
          source: "flclash:/Users/test/Library/Preferences/com.follow.clash.plist",
          sockPath: null,
        };
      }
      if (cmd === "mihomo_http") {
        if (args?.req?.path === "/version") {
          return { status: 200, body: JSON.stringify({ version: "v1.19.0", meta: true }) };
        }
        if (args?.req?.path === "/configs") {
          return { status: 200, body: JSON.stringify({ "mixed-port": 7899, mode: "rule" }) };
        }
      }
      throw new Error(`unexpected cmd ${cmd}`);
    });

    const state = await discoverAndProbe(undefined, "flclash");
    expect(state.status).toBe("connected");
    expect(state.message).toBe("已连上 FlClash");
    // Live mixed-port (7899) from GET /configs overrides static 7890
    expect(state.config?.mixedPort).toBe(7899);
    expect(state.config?.sockPath).toBeNull();
  });

  it("discoverAndProbe surfaces EC disabled guidance when FlClash EC is off", async () => {
    h.invoke.mockImplementation(async (cmd: string) => {
      if (cmd === "discover_mihomo_for_client") {
        return {
          host: "127.0.0.1",
          port: 9090,
          secret: "",
          mixedPort: 7890,
          source: "flclash-ec-disabled:/Users/test/Library/Preferences/com.follow.clash.plist",
          sockPath: null,
        };
      }
      throw new Error("connection refused");
    });

    const state = await discoverAndProbe(undefined, "flclash");
    expect(state.status).toBe("unreachable");
    expect(state.message).toContain("检测到 FlClash 未开启外部控制器");
  });
});

describe("0.2.4 inboundAuth & mixedPortDisabled detection in GET /configs", () => {
  beforeEach(() => {
    h.invoke.mockReset();
    (globalThis as unknown as { window?: Record<string, unknown> }).window = {
      __TAURI_INTERNALS__: {},
    };
  });

  it("flags inboundAuth=true when authentication is non-empty without loopback exemption", async () => {
    h.invoke.mockImplementation(async (cmd: string, args?: { req?: { path?: string } }) => {
      if (cmd === "discover_mihomo_for_client") {
        return {
          host: "127.0.0.1",
          port: 9090,
          secret: "fl-32-secret",
          mixedPort: 7890,
          source: "flclash:/Users/test/Library/Preferences/com.follow.clash.plist",
          sockPath: null,
        };
      }
      if (cmd === "mihomo_http") {
        if (args?.req?.path === "/version") {
          return { status: 200, body: JSON.stringify({ version: "v1.19.0", meta: true }) };
        }
        if (args?.req?.path === "/configs") {
          return {
            status: 200,
            body: JSON.stringify({
              "mixed-port": 7890,
              authentication: ["abcd1234:1234567890abcdef"],
              "skip-auth-prefixes": [],
            }),
          };
        }
      }
      throw new Error(`unexpected cmd ${cmd}`);
    });

    const state = await discoverAndProbe(undefined, "flclash");
    expect(state.status).toBe("connected");
    expect(state.config?.inboundAuth).toBe(true);
    expect(state.config?.mixedPortDisabled).toBe(false);
  });

  it("does not flag inboundAuth when 127.0.0.1/8 is in skip-auth-prefixes", async () => {
    h.invoke.mockImplementation(async (cmd: string, args?: { req?: { path?: string } }) => {
      if (cmd === "discover_mihomo_for_client") {
        return {
          host: "127.0.0.1",
          port: 9097,
          secret: "verge-secret",
          mixedPort: 7897,
          source: "verge",
          sockPath: null,
        };
      }
      if (cmd === "mihomo_http") {
        if (args?.req?.path === "/version") {
          return { status: 200, body: JSON.stringify({ version: "v1.19.0", meta: true }) };
        }
        if (args?.req?.path === "/configs") {
          return {
            status: 200,
            body: JSON.stringify({
              "mixed-port": 7897,
              authentication: ["user:pass"],
              "skip-auth-prefixes": ["127.0.0.1/8", "::1/128"],
            }),
          };
        }
      }
      throw new Error(`unexpected cmd ${cmd}`);
    });

    const state = await discoverAndProbe(undefined, "verge");
    expect(state.status).toBe("connected");
    expect(state.config?.inboundAuth).toBeUndefined();
    expect(state.config?.mixedPort).toBe(7897);
  });

  it("flags mixedPortDisabled=true when GET /configs reports mixed-port=0 and port=0", async () => {
    h.invoke.mockImplementation(async (cmd: string, args?: { req?: { path?: string } }) => {
      if (cmd === "discover_mihomo_for_client") {
        return {
          host: "127.0.0.1",
          port: 9090,
          secret: "fl-32-secret",
          mixedPort: 7890,
          source: "flclash:/Users/test/Library/Preferences/com.follow.clash.plist",
          sockPath: null,
        };
      }
      if (cmd === "mihomo_http") {
        if (args?.req?.path === "/version") {
          return { status: 200, body: JSON.stringify({ version: "v1.19.0", meta: true }) };
        }
        if (args?.req?.path === "/configs") {
          return {
            status: 200,
            body: JSON.stringify({
              "mixed-port": 0,
              port: 0,
              authentication: null,
            }),
          };
        }
      }
      throw new Error(`unexpected cmd ${cmd}`);
    });

    const state = await discoverAndProbe(undefined, "flclash");
    expect(state.status).toBe("connected");
    expect(state.config?.mixedPort).toBe(0);
    expect(state.config?.mixedPortDisabled).toBe(true);
  });
});
