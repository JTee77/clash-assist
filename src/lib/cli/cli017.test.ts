/**
 * 0.1.17：nodes / 筛选 / 进度 / 规则模式当前节点 / 取整 / os 闸门 / hint / help spec / install-shim。
 */
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { EgressReport } from "../egress";
import type { ConnectionState, ProxyNode } from "../mihomo";
import type { CheckAllData, CliProgressEvent, DiscoverData, NodesData } from "./dispatch";
import { CLI_ERROR_HINTS } from "./hints";
import { CLI_SPEC } from "./spec";
import { CLI_COMMANDS } from "./parse";
import { parseFilterBody, parsedFromServeJob } from "./serve";
import cliMd from "../../../docs/CLI.md?raw";

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

const { dispatchCli } = await import("./dispatch");
const { parseCliArgv } = await import("./parse");

const run = (argv: string[], opts?: import("./dispatch").CliDispatchOptions) =>
  dispatchCli(parseCliArgv(["egress-checker", "--cli", ...argv]), opts);

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


const many = (n: number): ProxyNode[] =>
  Array.from({ length: n }, (_, i) => ({
    ...node(i % 2 ? `日本 ${i}` : `香港 ${i}`),
    region: i % 2 ? "日本" : "中国香港",
  }));

describe("规则模式：currentProxy 来自节点列表", () => {
  it("discoverAndProbe 没给 currentProxy 时用 getProxies 的识别结果", async () => {
    h.discoverAndProbe.mockResolvedValue({ ...connected, currentProxy: null });
    h.getProxies.mockResolvedValue({
      nodes: [node("A"), node("B")],
      currentProxy: "B",
      usingMock: false,
      error: null,
      unauthorized: false,
    });
    const env = await run(["discover", "-c", "verge"]);
    const d = env.data as DiscoverData;
    expect(d.currentProxy).toBe("B");
    expect(d.currentProxyReason).toBeNull();
  });

  it("仍识别不到时给出 currentProxyReason", async () => {
    h.discoverAndProbe.mockResolvedValue({ ...connected, currentProxy: null });
    h.getProxies.mockResolvedValue({
      nodes: [node("A")],
      currentProxy: null,
      usingMock: false,
      error: null,
      unauthorized: false,
    });
    const d = (await run(["discover", "-c", "verge"])).data as DiscoverData;
    expect(d.currentProxy).toBeNull();
    expect(d.currentProxyReason).toMatch(/DIRECT|无法唯一判定/);
  });
});

describe("nodes", () => {
  beforeEach(() => {
    h.getProxies.mockResolvedValue({
      nodes: many(80),
      currentProxy: "香港 0",
      usingMock: false,
      error: null,
      unauthorized: false,
    });
  });

  it("不截断，标出当前节点", async () => {
    const env = await run(["nodes", "-c", "verge"]);
    expect(env.ok).toBe(true);
    const d = env.data as NodesData;
    expect(d.total).toBe(80);
    expect(d.count).toBe(80);
    expect(d.nodes[0]).toMatchObject({ name: "香港 0", current: true, lastDelay: null });
    expect(h.runNodeDiagnostics).not.toHaveBeenCalled();
  });

  it("--region 支持别名 / --match / --limit", async () => {
    let d = (await run(["nodes", "-c", "verge", "--region", "JP"])).data as NodesData;
    expect(d.count).toBe(40);
    expect(d.nodes.every((n) => n.region === "日本")).toBe(true);
    d = (await run(["nodes", "-c", "verge", "--region=香港", "--limit", "3"])).data as NodesData;
    expect(d.nodes.map((n) => n.name)).toEqual(["香港 0", "香港 2", "香港 4"]);
    d = (await run(["nodes", "-c", "verge", "--match", "日本 1,香港 2"])).data as NodesData;
    expect(d.nodes.every((n) => /日本 1|香港 2/.test(n.name))).toBe(true);
    expect(d.filter).toEqual({ region: null, match: "日本 1,香港 2", limit: null });
  });
});

describe("check all 子集", () => {
  it("--limit 1：只测 1 个并切回", async () => {
    const env = await run(["check", "all", "-c", "verge", "--limit", "1"]);
    expect(env.ok).toBe(true);
    const d = env.data as CheckAllData;
    expect(d.selection?.selected).toEqual(["A"]);
    expect(d.count).toBe(1);
    expect(h.runNodeDiagnostics).toHaveBeenCalledTimes(1);
    expect(h.restoreProxy).toHaveBeenCalledWith(connected.config, { group: "Proxy", now: "A" });
  });

  it("check nodes A B：按名解析", async () => {
    const env = await run(["check", "nodes", "B", "A", "-c", "verge"]);
    expect(env.ok).toBe(true);
    expect((env.data as CheckAllData).selection?.names).toEqual(["B", "A"]);
    expect((env.data as CheckAllData).count).toBe(2);
  });

  it("check nodes 里有不存在的名字 → node_not_found，不碰门槛", async () => {
    const env = await run(["check", "nodes", "A", "Zzz", "-c", "verge"]);
    expect(env.error?.code).toBe("node_not_found");
    expect(h.runLightGate).not.toHaveBeenCalled();
    expect(h.switchProxy).not.toHaveBeenCalled();
  });

  it("筛选为空 → no_nodes_matched", async () => {
    const env = await run(["check", "all", "-c", "verge", "--match", "nothing"]);
    expect(env.error?.code).toBe("no_nodes_matched");
    expect(env.error?.hint).toBeTruthy();
    expect(h.runLightGate).not.toHaveBeenCalled();
  });

  it("未筛选时 selection 为 null（兼容）", async () => {
    const env = await run(["check", "all", "-c", "verge"]);
    expect((env.data as CheckAllData).selection).toBeNull();
  });
});

describe("进度事件", () => {
  it("check all：start → stage → node_done → restore → done", async () => {
    const evs: CliProgressEvent[] = [];
    await run(["check", "all", "-c", "verge", "--limit", "2"], { onProgress: (e) => evs.push(e) });
    const types = evs.map((e) => e.type);
    expect(types[0]).toBe("start");
    expect(types[types.length - 1]).toBe("done");
    const tests = evs.filter((e) => e.type === "stage" && e.stage === "test");
    expect(tests.map((e) => [e.index, e.total, e.nodeName])).toEqual([
      [1, 2, "A"],
      [2, 2, "B"],
    ]);
    const done = evs.filter((e) => e.type === "node_done");
    expect(done.map((e) => e.nodeName)).toEqual(["A", "B"]);
    expect(done[0]).toMatchObject({ index: 1, total: 2 });
    expect(evs.some((e) => e.stage === "restore")).toBe(true);
    expect(evs.every((e) => e.command === "check" && typeof e.ts === "string")).toBe(true);
  });

  it("discover / nodes 不发进度", async () => {
    const evs: CliProgressEvent[] = [];
    await run(["discover", "-c", "verge"], { onProgress: (e) => evs.push(e) });
    await run(["nodes", "-c", "verge"], { onProgress: (e) => evs.push(e) });
    expect(evs).toEqual([]);
  });

  it("--no-progress 被解析", () => {
    expect(parseCliArgv(["x", "--cli", "env", "--no-progress"]).progress).toBe(false);
    expect(parseCliArgv(["x", "--cli", "env", "-q"]).progress).toBe(false);
    expect(parseCliArgv(["x", "--cli", "env"]).progress).toBe(true);
  });
});

describe("取整", () => {
  it("cards 里的长小数保留 2 位，totalScore 1 位", async () => {
    h.runNodeDiagnostics.mockImplementation(async () => ({
      ...fakeReport(),
      cards: [
        {
          id: "bandwidth",
          title: "带宽",
          level: "pass",
          conclusion: "ok",
          metrics: { downMbps: 12.3456789, latencyMs: 101 },
        },
      ],
    }));
    const env = await run(["check", "current", "-c", "verge"]);
    const d = env.data as { cards: { metrics: { downMbps: number; latencyMs: number } }[]; score: { totalScore: number } };
    expect(d.cards[0]!.metrics.downMbps).toBe(12.35);
    expect(d.cards[0]!.metrics.latencyMs).toBe(101);
    expect(String(d.score.totalScore)).toMatch(/^\d+(\.\d)?$/);
  });
});

describe("macOS < 14 快速失败", () => {
  const old = { os: "macos", version: "13.6", major: 13 };
  it("check / env → os_unsupported，不碰网络", async () => {
    for (const argv of [["check", "current"], ["check", "all"], ["env"]]) {
      const env = await run([...argv, "-c", "verge"], { osInfo: old });
      expect(env.error?.code).toBe("os_unsupported");
      expect(env.error?.hint).toContain("macOS 14");
    }
    noNetworkTouched();
  });
  it("discover 不受影响；14+ 正常", async () => {
    expect((await run(["discover", "-c", "verge"], { osInfo: old })).ok).toBe(true);
    const env = await run(["check", "current", "-c", "verge"], {
      osInfo: { os: "macos", version: "14.0", major: 14 },
    });
    expect(env.ok).toBe(true);
  });
});

describe("hint / help spec", () => {
  it("docs/CLI.md 错误码表里每个 code 都有 hint", () => {
    const md = cliMd;
    const section = md.split("### 错误码")[1]!.split("\n## ")[0]!;
    const codes = [...section.matchAll(/^\| `([a-z_]+)`/gm)].map((m) => m[1]!);
    expect(codes.length).toBeGreaterThan(20);
    for (const c of codes) expect(CLI_ERROR_HINTS[c], c).toBeTruthy();
  });

  it("参数错误也带 hint", async () => {
    const env = await run(["discover", "--verbose"]);
    expect(env.error?.hint).toBeTruthy();
  });

  it("help 的 data.spec 覆盖全部命令", async () => {
    const env = await run(["help", "--json"]);
    const spec = (env.data as { spec: typeof CLI_SPEC }).spec;
    const names = new Set(spec.commands.map((c) => c.name.split(" ")[0]));
    for (const c of CLI_COMMANDS) expect(names.has(c), c).toBe(true);
    expect(spec.errorCodes.os_unsupported).toBeTruthy();
  });
});

describe("install-shim", () => {
  it("没有实现（serve / 测试环境）→ shim_unavailable", async () => {
    const env = await run(["install-shim"]);
    expect(env.error?.code).toBe("shim_unavailable");
  });
  it("成功：透传结果并给出下一步", async () => {
    const installShim = vi.fn().mockResolvedValue({
      path: "/u/.local/bin/egress-checker",
      dir: "/u/.local/bin",
      target: "/Applications/Egress Checker.app/Contents/MacOS/egress-checker",
      created: true,
      onPath: false,
      targetIsDevBuild: false,
    });
    const env = await run(["install-shim", "--dir", "/u/.local/bin"], { installShim });
    expect(installShim).toHaveBeenCalledWith("/u/.local/bin");
    expect(env.ok).toBe(true);
    expect((env.data as { next: string }).next).toContain("PATH");
    noNetworkTouched();
  });
  it("Rust 报错对象透传 code", async () => {
    const installShim = vi.fn().mockRejectedValue({ code: "shim_exists", message: "已存在" });
    const env = await run(["install-shim"], { installShim });
    expect(env.error).toMatchObject({ code: "shim_exists", message: "已存在" });
    expect(env.error?.hint).toBeTruthy();
  });
});

describe("解析：选项适用范围", () => {
  const p = (...a: string[]) => parseCliArgv(["x", "--cli", ...a]);
  it("筛选只用于 nodes / check all / check nodes", () => {
    expect(p("discover", "--limit", "2").parseError?.code).toBe("option_not_applicable");
    expect(p("check", "node", "A", "--region", "JP").parseError?.code).toBe("option_not_applicable");
    expect(p("check", "all", "--limit", "0").parseError?.code).toBe("invalid_limit");
    expect(p("check", "all", "--limit=2").filter.limit).toBe(2);
    expect(p("check", "nodes").parseError?.code).toBe("node_name_required");
    expect(p("check", "nodes", "A", "B C").checkTarget).toEqual({ kind: "all", names: ["A", "B C"] });
    expect(p("discover", "--dir", "/x").parseError?.code).toBe("option_not_applicable");
    expect(p("install-shim", "--dir", "/x").dir).toBe("/x");
    expect(p("nodes", "--match").parseError?.code).toBe("missing_option_value");
  });
});

describe("serve body", () => {
  it("parseFilterBody", () => {
    expect(parseFilterBody("", true)).toEqual({ ok: true, filter: { region: null, match: null, limit: null }, names: [] });
    expect(parseFilterBody('{"region":["JP","HK"],"limit":2,"names":["A"]}', true)).toEqual({
      ok: true,
      filter: { region: "JP,HK", match: null, limit: 2 },
      names: ["A"],
    });
    expect(parseFilterBody('{"names":["A"]}', false).ok).toBe(false);
    expect(parseFilterBody('{"limit":0}', true).ok).toBe(false);
    expect(parseFilterBody("[1]", true).ok).toBe(false);
  });
  it("路由映射", () => {
    const base = parseCliArgv(["x", "--cli", "serve", "--mock"]);
    const a = parsedFromServeJob(base, { route: "check/all", body: '{"names":["A"],"limit":1}' });
    expect(a.parsed.checkTarget).toEqual({ kind: "all", names: ["A"] });
    expect(a.parsed.filter.limit).toBe(1);
    const n = parsedFromServeJob(base, { route: "nodes", body: '{"match":"港"}' });
    expect(n.parsed.command).toBe("nodes");
    expect(n.parsed.filter.match).toBe("港");
    expect(parsedFromServeJob(base, { route: "nodes", body: "{" }).early?.error?.code).toBe("invalid_body");
  });
});

describe("文案里的长小数", () => {
  it("roundText 只动 4+ 位小数", async () => {
    const { roundText } = await import("./dispatch");
    expect(roundText("下行约 4.745562254259502 Mbps，偏慢")).toBe("下行约 4.75 Mbps，偏慢");
    expect(roundText("IP 1.2.3.4 版本 0.1.17 延迟 12.5 ms")).toBe("IP 1.2.3.4 版本 0.1.17 延迟 12.5 ms");
  });
});
