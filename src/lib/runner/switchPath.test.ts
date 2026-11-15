import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ConnectionState, ProxyNode } from "../mihomo";
import type { RunnerHooks } from "./types";

const h = vi.hoisted(() => ({
  runNodeDiagnostics: vi.fn(),
  resolveSelectorSnapshot: vi.fn(),
  findSelectorGroup: vi.fn(),
  switchProxy: vi.fn(),
  restoreProxy: vi.fn(),
  probeDelay: vi.fn(),
  confirmTrafficLeaf: vi.fn(),
}));

vi.mock("../egress", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../egress")>()),
  runNodeDiagnostics: h.runNodeDiagnostics,
}));

vi.mock("../mihomo", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../mihomo")>()),
  resolveSelectorSnapshot: h.resolveSelectorSnapshot,
  findSelectorGroup: h.findSelectorGroup,
  switchProxy: h.switchProxy,
  restoreProxy: h.restoreProxy,
  probeDelay: h.probeDelay,
  confirmTrafficLeaf: h.confirmTrafficLeaf,
}));

const { testAll } = await import("./testAll");
const { testOne } = await import("./testOne");

const HK17 = "香港高速3-HK17";
const US82 = "香港高速3-美国82 gemini";
const MAIN = "😣 节点选择";

function connection(mode: string): ConnectionState {
  return {
    status: "connected",
    message: "已连上",
    config: {
      host: "127.0.0.1",
      port: 9097,
      secret: "",
      mixedPort: 7897,
      source: "test",
      sockPath: null,
      mode,
    },
    currentProxy: HK17,
    usingMock: false,
  };
}

const node = (name: string): ProxyNode => ({
  name,
  type: "Hysteria2",
  region: "未知",
  raw: { name, type: "Hysteria2" },
});

function hooks() {
  const scores: string[] = [];
  const gates: string[] = [];
  const hints: Array<string | null> = [];
  const api: RunnerHooks = {
    onProgress: () => {},
    onNodeCards: () => {},
    onUpsertScore: (s) => {
      scores.push(`${s.nodeName}:${s.blurb}`);
    },
    onHint: (m) => hints.push(m),
    onGate: (g) => {
      if (!g.ok) gates.push(g.message);
    },
    onRestoreError: () => {},
  };
  return { api, scores, gates, hints };
}

describe("测节点不会在切错组或直连模式下记录出口", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    delete (globalThis as { window?: unknown }).window;
    h.runNodeDiagnostics.mockResolvedValue({
      ranAt: "2026-11-16T00:00:00.000Z",
      cards: [],
      exitIp: { ip: "42.200.231.208" },
      note: "should-not-run",
    });
    h.resolveSelectorSnapshot.mockResolvedValue({ group: MAIN, now: HK17 });
    h.findSelectorGroup.mockResolvedValue(MAIN);
    h.switchProxy.mockResolvedValue(true);
    h.restoreProxy.mockResolvedValue(true);
    h.probeDelay.mockResolvedValue(80);
    h.confirmTrafficLeaf.mockResolvedValue(true);
  });

  it("直连模式：测全部不切换、不跑诊断", async () => {
    const bag = hooks();
    const ok = await testAll(
      {
        connection: connection("direct"),
        nodes: [node(HK17), node(US82)],
        forceMock: false,
        mixedPort: 7897,
        ensureGate: async () => ({ ok: true, message: "ok" }),
        shouldAbort: () => false,
      },
      bag.api,
    );
    expect(ok).toBe(false);
    expect(bag.gates[0]).toMatch(/直连模式/);
    expect(h.switchProxy).not.toHaveBeenCalled();
    expect(h.runNodeDiagnostics).not.toHaveBeenCalled();
  });

  it("直连模式：测单个同样直接失败", async () => {
    const bag = hooks();
    const ok = await testOne(
      {
        node: node(US82),
        connection: connection("direct"),
        forceMock: false,
        mixedPort: 7897,
        ensureGate: async () => ({ ok: true, message: "ok" }),
      },
      bag.api,
    );
    expect(ok).toBe(false);
    expect(bag.hints[bag.hints.length - 1]).toMatch(/直连模式/);
    expect(h.switchProxy).not.toHaveBeenCalled();
    expect(h.runNodeDiagnostics).not.toHaveBeenCalled();
  });

  it("切换未落到流量组：不跑诊断，并仍按主选择组切回", async () => {
    h.switchProxy.mockResolvedValue(false);
    const bag = hooks();
    const ok = await testAll(
      {
        connection: connection("rule"),
        nodes: [node(US82)],
        forceMock: false,
        mixedPort: 7897,
        ensureGate: async () => ({ ok: true, message: "ok" }),
        shouldAbort: () => false,
      },
      bag.api,
    );
    expect(ok).toBe(true);
    expect(h.runNodeDiagnostics).not.toHaveBeenCalled();
    expect(h.switchProxy).toHaveBeenCalledWith(
      expect.objectContaining({ mode: "rule" }),
      MAIN,
      US82,
    );
    expect(bag.scores.some((s) => s.includes("切换失败"))).toBe(true);
    expect(h.restoreProxy).toHaveBeenCalledWith(
      expect.anything(),
      expect.objectContaining({ group: MAIN, now: HK17 }),
    );
  });

  it("测单个切换失败时不跑诊断", async () => {
    h.switchProxy.mockResolvedValue(false);
    const bag = hooks();
    const ok = await testOne(
      {
        node: node(US82),
        connection: connection("rule"),
        forceMock: false,
        mixedPort: 7897,
        ensureGate: async () => ({ ok: true, message: "ok" }),
      },
      bag.api,
    );
    expect(ok).toBe(false);
    expect(h.runNodeDiagnostics).not.toHaveBeenCalled();
    expect(bag.hints[bag.hints.length - 1]).toMatch(/切换失败/);
    expect(h.restoreProxy).toHaveBeenCalled();
  });
});
