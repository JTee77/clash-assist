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
  refreshClashMode: vi.fn(),
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
  refreshClashMode: h.refreshClashMode,
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
  const progress: string[] = [];
  let cleared = false;
  const api: RunnerHooks = {
    onProgress: (p) => {
      if (p?.text) progress.push(p.text);
    },
    onNodeCards: () => {},
    onUpsertScore: (s) => {
      scores.push(`${s.nodeName}:${s.blurb}`);
    },
    onScores: (list) => {
      if (list.length === 0) cleared = true;
    },
    onHint: (m) => hints.push(m),
    onGate: (g) => {
      if (!g.ok) gates.push(g.message);
    },
    onRestoreError: () => {},
  };
  return { api, scores, gates, hints, progress, wasCleared: () => cleared };
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
    h.refreshClashMode.mockImplementation(async (config: { mode?: string }) => config.mode);
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

  it("Windows FlClash 规则模式切换失败后仍切回原先节点", async () => {
    h.switchProxy.mockResolvedValue(false);
    const bag = hooks();
    const conn = connection("rule");
    conn.config = {
      ...conn.config!,
      port: 9090,
      secret: "",
      mixedPort: 7890,
      source:
        "flclash:C:\\Users\\ada\\AppData\\Roaming\\com.follow\\clash\\shared_preferences.json",
      sockPath: null,
    };
    const ok = await testAll(
      {
        connection: conn,
        nodes: [node(US82)],
        forceMock: false,
        mixedPort: 7890,
        ensureGate: async () => ({ ok: true, message: "ok" }),
        shouldAbort: () => false,
      },
      bag.api,
    );
    expect(ok).toBe(true);
    expect(h.switchProxy).toHaveBeenCalledWith(
      expect.objectContaining({ port: 9090, sockPath: null, mode: "rule" }),
      MAIN,
      US82,
    );
    expect(h.restoreProxy).toHaveBeenCalledWith(
      expect.objectContaining({ source: expect.stringContaining("flclash:") }),
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

const UNSAFE = "没法安全地临时切换";

describe("停止不会被当成无法安全切换", () => {
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
    h.refreshClashMode.mockImplementation(async (config: { mode?: string }) => config.mode);
  });

  function runAll(opts: {
    abortAt: "gate" | "mode" | "snapshot" | "group" | "precheck" | "after-switch" | "never";
    snapshot?: "ok" | "null";
  }) {
    let abort = false;
    const shouldAbort = () => abort;
    if (opts.abortAt === "mode") {
      h.refreshClashMode.mockImplementation(async () => {
        abort = true;
        throw new Error("mode read failed");
      });
    }
    if (opts.abortAt === "snapshot") {
      h.resolveSelectorSnapshot.mockImplementation(async () => {
        abort = true;
        throw new Error("snapshot read failed");
      });
    }
    if (opts.abortAt === "group") {
      h.resolveSelectorSnapshot.mockResolvedValue(null);
      h.findSelectorGroup.mockImplementation(async () => {
        abort = true;
        throw new Error("group read failed");
      });
    }
    if (opts.snapshot === "null") {
      h.resolveSelectorSnapshot.mockResolvedValue(null);
      h.findSelectorGroup.mockResolvedValue(null);
    }
    if (opts.abortAt === "precheck") {
      h.probeDelay.mockImplementation(async () => {
        abort = true;
        throw new Error("delay read failed");
      });
    }
    if (opts.abortAt === "after-switch") {
      h.switchProxy.mockImplementation(async () => {
        abort = true;
        return true;
      });
    }
    const bag = hooks();
    const done = testAll(
      {
        connection: connection("rule"),
        nodes: [node(HK17), node(US82)],
        forceMock: false,
        mixedPort: 7897,
        ensureGate: async () => {
          if (opts.abortAt === "gate") abort = true;
          return { ok: true, message: "ok" };
        },
        shouldAbort,
      },
      bag.api,
    );
    return { done, bag };
  }

  it("条件检查中停止：不检测、不切换、不报无法安全切换，也不清掉旧成绩", async () => {
    const { done, bag } = runAll({ abortAt: "gate" });
    await expect(done).resolves.toBe(false);
    expect(h.resolveSelectorSnapshot).not.toHaveBeenCalled();
    expect(h.switchProxy).not.toHaveBeenCalled();
    expect(h.runNodeDiagnostics).not.toHaveBeenCalled();
    expect(h.restoreProxy).not.toHaveBeenCalled();
    expect(bag.hints.some((m) => m?.includes(UNSAFE))).toBe(false);
    expect(bag.gates.some((m) => m.includes("测全部节点时出错"))).toBe(false);
    expect(bag.progress.some((t) => t.includes("无法安全切换"))).toBe(false);
    expect(bag.wasCleared()).toBe(false);
  });

  it("停止落在模式读取失败上：不当成模式阻断，也不开测", async () => {
    const { done, bag } = runAll({ abortAt: "mode" });
    await expect(done).resolves.toBe(false);
    expect(h.resolveSelectorSnapshot).not.toHaveBeenCalled();
    expect(h.runNodeDiagnostics).not.toHaveBeenCalled();
    expect(h.switchProxy).not.toHaveBeenCalled();
    expect(bag.hints.some((m) => m?.includes(UNSAFE) || m?.includes("直连模式"))).toBe(false);
    expect(bag.gates).toEqual([]);
    expect(bag.wasCleared()).toBe(false);
  });

  it("停止落在快照或策略组读取失败上：不当成无法安全切换", async () => {
    for (const abortAt of ["snapshot", "group"] as const) {
      vi.clearAllMocks();
      h.restoreProxy.mockResolvedValue(true);
      h.refreshClashMode.mockImplementation(async (config: { mode?: string }) => config.mode);
      h.probeDelay.mockResolvedValue(80);
      const { done, bag } = runAll({ abortAt });
      await expect(done).resolves.toBe(false);
      expect(h.switchProxy).not.toHaveBeenCalled();
      expect(h.runNodeDiagnostics).not.toHaveBeenCalled();
      expect(h.restoreProxy).not.toHaveBeenCalled();
      expect(bag.hints.some((m) => m?.includes(UNSAFE))).toBe(false);
      expect(bag.gates.some((m) => m.includes("测全部节点时出错"))).toBe(false);
      expect(bag.progress.some((t) => t.includes("无法安全切换"))).toBe(false);
      expect(bag.wasCleared()).toBe(false);
    }
  });

  it("预检进行中失败且已经停止：不再进入检测", async () => {
    const { done, bag } = runAll({ abortAt: "precheck" });
    await expect(done).resolves.toBe(false);
    expect(h.switchProxy).not.toHaveBeenCalled();
    expect(h.runNodeDiagnostics).not.toHaveBeenCalled();
    expect(bag.hints.some((m) => m?.includes(UNSAFE))).toBe(false);
    expect(bag.gates.some((m) => m.includes("测全部节点时出错"))).toBe(false);
  });

  it("已经切换之后停止：切回原节点，不再检测下一个", async () => {
    const { done, bag } = runAll({ abortAt: "after-switch" });
    await expect(done).resolves.toBe(false);
    expect(h.switchProxy).toHaveBeenCalledTimes(1);
    expect(h.runNodeDiagnostics).not.toHaveBeenCalled();
    expect(h.restoreProxy).toHaveBeenCalledWith(
      expect.anything(),
      expect.objectContaining({ group: MAIN, now: HK17 }),
    );
    expect(bag.hints.some((m) => m?.includes(UNSAFE))).toBe(false);
  });

  it("真的读不到当前节点时仍只检测当前节点", async () => {
    const { done, bag } = runAll({ abortAt: "never", snapshot: "null" });
    await expect(done).resolves.toBe(true);
    expect(h.switchProxy).not.toHaveBeenCalled();
    expect(h.restoreProxy).not.toHaveBeenCalled();
    expect(h.runNodeDiagnostics).toHaveBeenCalledTimes(1);
    expect(bag.hints.some((m) => m?.includes(UNSAFE))).toBe(true);
    expect(bag.progress.some((t) => t.includes("无法安全切换"))).toBe(true);
  });

  it("测单个在条件检查或切换失败被停止时不误报、切过则切回", async () => {
    let abort = false;
    const bag = hooks();
    h.refreshClashMode.mockImplementation(async () => {
      abort = true;
      throw new Error("mode read failed");
    });
    const stopped = await testOne(
      {
        node: node(US82),
        connection: connection("rule"),
        forceMock: false,
        mixedPort: 7897,
        ensureGate: async () => ({ ok: true, message: "ok" }),
        shouldAbort: () => abort,
      },
      bag.api,
    );
    expect(stopped).toBe(false);
    expect(h.switchProxy).not.toHaveBeenCalled();
    expect(h.runNodeDiagnostics).not.toHaveBeenCalled();
    expect(h.restoreProxy).not.toHaveBeenCalled();
    expect(bag.hints.some((m) => m && m.length > 0)).toBe(false);

    abort = false;
    h.refreshClashMode.mockImplementation(async (config: { mode?: string }) => config.mode);
    h.switchProxy.mockImplementation(async () => {
      abort = true;
      return true;
    });
    const afterSwitch = hooks();
    const switched = await testOne(
      {
        node: node(US82),
        connection: connection("rule"),
        forceMock: false,
        mixedPort: 7897,
        ensureGate: async () => ({ ok: true, message: "ok" }),
        shouldAbort: () => abort,
      },
      afterSwitch.api,
    );
    expect(switched).toBe(false);
    expect(h.runNodeDiagnostics).not.toHaveBeenCalled();
    expect(h.restoreProxy).toHaveBeenCalledWith(
      expect.anything(),
      expect.objectContaining({ group: MAIN, now: HK17 }),
    );
    expect(afterSwitch.hints.some((m) => m?.includes("切换失败"))).toBe(false);
  });
});
