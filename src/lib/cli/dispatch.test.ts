/**
 * dispatchCli 端到端（mock 掉网络/控制器）：闸门、未知命令、门槛、切回、中断。
 */
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { EgressReport } from "../egress";
import type { ConnectionState, ProxyNode } from "../mihomo";
import type { CheckAllData, CheckNodeData } from "./dispatch";

const h = vi.hoisted(() => ({
  runNodeDiagnostics: vi.fn(),
  runLightGate: vi.fn(),
  discoverAndProbe: vi.fn(),
  getProxies: vi.fn(),
  resolveSelectorSnapshot: vi.fn(),
  findSelectorGroup: vi.fn(),
  switchProxy: vi.fn(),
  restoreProxy: vi.fn(),
  probeDelay: vi.fn(),
}));

vi.mock("../egress", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../egress")>()),
  runNodeDiagnostics: h.runNodeDiagnostics,
}));
vi.mock("../score", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../score")>()),
  runLightGate: h.runLightGate,
}));
vi.mock("../mihomo", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../mihomo")>()),
  discoverAndProbe: h.discoverAndProbe,
  getProxies: h.getProxies,
  resolveSelectorSnapshot: h.resolveSelectorSnapshot,
  findSelectorGroup: h.findSelectorGroup,
  switchProxy: h.switchProxy,
  restoreProxy: h.restoreProxy,
  probeDelay: h.probeDelay,
}));

const { dispatchCli, CLI_ABORT_MESSAGE } = await import("./dispatch");
const { parseCliArgv } = await import("./parse");

const run = (argv: string[], opts?: { shouldAbort?: () => boolean }) =>
  dispatchCli(parseCliArgv(["clash-assist", "--cli", ...argv]), opts);

const node = (name: string): ProxyNode => ({
  name,
  type: "ss",
  region: "未知",
  raw: { name, type: "ss" },
});

const fakeReport = (): EgressReport => ({
  ranAt: "2026-10-06T00:00:00.000Z",
  cards: [],
  exitIp: null,
  gemini: null,
  chatgpt: null,
  latencyMs: null,
  note: "fake",
});

const connected: ConnectionState = {
  status: "connected",
  message: "已连上",
  config: {
    host: "127.0.0.1",
    port: 9097,
    secret: "s3cret",
    mixedPort: 7897,
    source: "test",
    sockPath: null,
  },
  currentProxy: "A",
  usingMock: false,
  proxiesError: null,
};

function noNetworkTouched() {
  expect(h.discoverAndProbe).not.toHaveBeenCalled();
  expect(h.runNodeDiagnostics).not.toHaveBeenCalled();
  expect(h.runLightGate).not.toHaveBeenCalled();
  expect(h.switchProxy).not.toHaveBeenCalled();
}

beforeEach(() => {
  vi.clearAllMocks();
  h.runNodeDiagnostics.mockImplementation(async () => fakeReport());
  h.runLightGate.mockResolvedValue({ ok: true, message: "门槛通过" });
  h.discoverAndProbe.mockResolvedValue(connected);
  h.getProxies.mockResolvedValue({
    nodes: [node("A"), node("B")],
    currentProxy: "A",
    usingMock: false,
    error: null,
    unauthorized: false,
  });
  h.resolveSelectorSnapshot.mockResolvedValue({ group: "Proxy", now: "A" });
  h.findSelectorGroup.mockResolvedValue("Proxy");
  h.switchProxy.mockResolvedValue(true);
  h.restoreProxy.mockResolvedValue(true);
  h.probeDelay.mockResolvedValue(100);
});

describe("参数 / 闸门错误：ok:false，且不碰网络", () => {
  it("未知子命令", async () => {
    const env = await run(["frobnicate"]);
    expect(env.ok).toBe(false);
    expect(env.command).toBe("frobnicate");
    expect(env.error?.code).toBe("unknown_command");
    noNetworkTouched();
  });

  it("未知选项", async () => {
    const env = await run(["discover", "--client", "verge", "--verbose"]);
    expect(env.ok).toBe(false);
    expect(env.error?.code).toBe("unknown_option");
    noNetworkTouched();
  });

  it("缺 --client", async () => {
    const env = await run(["discover"]);
    expect(env.error?.code).toBe("client_required");
    noNetworkTouched();
  });

  it("-c flclash 被拒（真实与 --mock 都一样）", async () => {
    for (const extra of [[], ["--mock"]]) {
      const env = await run(["check", "all", "-c", "flclash", ...extra]);
      expect(env.ok).toBe(false);
      expect(env.error?.code).toBe("client_unsupported");
      expect(env.error?.message).toContain("当前仅支持 Clash Verge");
    }
    noNetworkTouched();
  });

  it("help 永远 ok", async () => {
    const env = await run(["help"]);
    expect(env.ok).toBe(true);
    expect((env.data as { text: string }).text).toContain("0.2.0");
  });
});

describe("discover", () => {
  it("不回显密钥；节点截断到 50 并标注", async () => {
    h.getProxies.mockResolvedValue({
      nodes: Array.from({ length: 60 }, (_, i) => node(`N${i}`)),
      currentProxy: "N0",
      usingMock: false,
      error: null,
      unauthorized: false,
    });
    const env = await run(["discover", "-c", "verge"]);
    expect(env.ok).toBe(true);
    const d = env.data as { nodeCount: number; nodes: unknown[]; nodesTruncated: boolean; config: Record<string, unknown> };
    expect(d.nodeCount).toBe(60);
    expect(d.nodes).toHaveLength(50);
    expect(d.nodesTruncated).toBe(true);
    expect(JSON.stringify(env)).not.toContain("s3cret");
  });

  it("读节点失败时给出 nodesError（不静默成空列表）", async () => {
    h.getProxies.mockResolvedValue({
      nodes: [],
      currentProxy: null,
      usingMock: false,
      error: "连接被拒绝",
      unauthorized: true,
    });
    const env = await run(["discover", "-c", "verge"]);
    expect((env.data as { nodesError: string }).nodesError).toBe("连接被拒绝");
  });
});

describe("check current（--mock）", () => {
  it("ok + target current", async () => {
    const env = await run(["check", "current", "--mock"]);
    expect(env.ok).toBe(true);
    expect(env.data).toMatchObject({ target: "current", clientId: "verge" });
    expect(h.discoverAndProbe).not.toHaveBeenCalled();
  });
});

describe("check node", () => {
  it("门槛不过 → gate_failed，不切换不深测", async () => {
    h.runLightGate.mockResolvedValue({ ok: false, message: "连不上VPN软件" });
    const env = await run(["check", "node", "B", "-c", "verge"]);
    expect(env.ok).toBe(false);
    expect(env.error).toMatchObject({ code: "gate_failed", message: "连不上VPN软件" });
    expect(h.switchProxy).not.toHaveBeenCalled();
    expect(h.runNodeDiagnostics).not.toHaveBeenCalled();
  });

  it("找不到节点 → node_not_found，不切换", async () => {
    const env = await run(["check", "node", "火星", "-c", "verge"]);
    expect(env.error?.code).toBe("node_not_found");
    expect(h.switchProxy).not.toHaveBeenCalled();
  });

  it("切换 → 测 → 切回成功：ok，restoreError=null", async () => {
    const env = await run(["check", "node", "B", "-c", "verge"]);
    expect(env.ok).toBe(true);
    const d = env.data as CheckNodeData;
    expect(d.nodeName).toBe("B");
    expect(d.restoreError).toBeNull();
    expect(d.score).not.toBeNull();
    expect(h.restoreProxy).toHaveBeenCalledWith(connected.config, { group: "Proxy", now: "A" });
  });

  it("切回失败 → restore_failed；restoreError 与 hint 分开", async () => {
    h.restoreProxy.mockResolvedValue(false);
    const env = await run(["check", "node", "B", "-c", "verge"]);
    expect(env.ok).toBe(false);
    expect(env.error?.code).toBe("restore_failed");
    const d = env.data as CheckNodeData;
    expect(d.restoreError).toBe("没能切回原先节点「A」。");
    expect(d.hint).toBeNull();
    expect(d.score).not.toBeNull();
  });

  it("切过但没记下原节点 → restore_failed（以前会静默）", async () => {
    h.resolveSelectorSnapshot.mockResolvedValue(null);
    const env = await run(["check", "node", "B", "-c", "verge"]);
    expect(env.error?.code).toBe("restore_failed");
    expect((env.data as CheckNodeData).restoreError).toBe("没能切回原先节点。");
  });

  it("切换失败 → check_failed，带 hint", async () => {
    h.switchProxy.mockResolvedValue(false);
    const env = await run(["check", "node", "B", "-c", "verge"]);
    expect(env.error?.code).toBe("check_failed");
    expect(env.error?.message).toBe("切换失败，没法测这个节点。");
    expect((env.data as CheckNodeData).restoreError).toBeNull();
  });
});

describe("check all", () => {
  it("全部测完并切回：ok", async () => {
    const env = await run(["check", "all", "-c", "verge"]);
    expect(env.ok).toBe(true);
    const d = env.data as CheckAllData;
    expect(d.count).toBe(2);
    expect(d.aborted).toBe(false);
    expect(d.restoreError).toBeNull();
    expect(h.restoreProxy).toHaveBeenCalledTimes(1);
  });

  it("切回失败 → restore_failed，restoreError 单独字段，hint 不重复", async () => {
    h.restoreProxy.mockResolvedValue(false);
    const env = await run(["check", "all", "-c", "verge"]);
    expect(env.ok).toBe(false);
    expect(env.error).toMatchObject({ code: "restore_failed", message: "没能切回原先节点「A」。" });
    const d = env.data as CheckAllData;
    expect(d.restoreError).toBe("没能切回原先节点「A」。");
    expect(d.hint).toBeNull();
    expect(d.count).toBe(2);
  });

  it("中断：测完当前节点后停止，并切回原节点", async () => {
    let aborted = false;
    h.runNodeDiagnostics.mockImplementation(async () => {
      aborted = true; // 第一个节点测试中收到信号
      return fakeReport();
    });
    const env = await run(["check", "all", "-c", "verge"], {
      shouldAbort: () => aborted,
    });
    expect(env.ok).toBe(false);
    expect(env.error).toMatchObject({ code: "aborted", message: CLI_ABORT_MESSAGE });
    const d = env.data as CheckAllData;
    expect(d.aborted).toBe(true);
    expect(d.count).toBe(1);
    expect(d.restoreError).toBeNull();
    expect(h.runNodeDiagnostics).toHaveBeenCalledTimes(1);
    expect(h.restoreProxy).toHaveBeenCalledWith(connected.config, { group: "Proxy", now: "A" });
  });

  it("中断 + 切回失败：restore_failed 优先", async () => {
    h.restoreProxy.mockResolvedValue(false);
    let aborted = false;
    h.runNodeDiagnostics.mockImplementation(async () => {
      aborted = true;
      return fakeReport();
    });
    const env = await run(["check", "all", "-c", "verge"], { shouldAbort: () => aborted });
    expect(env.error?.code).toBe("restore_failed");
    expect((env.data as CheckAllData).aborted).toBe(true);
  });

  it("--mock 中断：ok:false aborted，不切换", async () => {
    let aborted = false;
    h.runNodeDiagnostics.mockImplementation(async () => {
      aborted = true;
      return fakeReport();
    });
    const env = await run(["check", "all", "--mock"], { shouldAbort: () => aborted });
    expect(env.error?.code).toBe("aborted");
    expect(h.switchProxy).not.toHaveBeenCalled();
    expect(h.restoreProxy).not.toHaveBeenCalled();
  });

  it("门槛不过 → gate_failed", async () => {
    h.runLightGate.mockResolvedValue({ ok: false, message: "海外探测全部失败" });
    const env = await run(["check", "all", "-c", "verge"]);
    expect(env.error?.code).toBe("gate_failed");
    expect(h.switchProxy).not.toHaveBeenCalled();
  });

  it("没有节点可测 → check_failed（不是空的 ok）", async () => {
    h.getProxies.mockResolvedValue({
      nodes: [],
      currentProxy: null,
      usingMock: false,
      error: "节点列表为空",
      unauthorized: false,
    });
    h.discoverAndProbe.mockResolvedValue({ ...connected, currentProxy: null });
    const env = await run(["check", "all", "-c", "verge"]);
    expect(env.ok).toBe(false);
    expect(env.error?.code).toBe("check_failed");
    expect((env.data as CheckAllData).nodesError).toBe("节点列表为空");
  });
});
