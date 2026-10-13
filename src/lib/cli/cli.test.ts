import { describe, it, expect } from "vitest";
import { normalizeClientId, type ProxyNode } from "../mihomo";
import {
  CLI_ONLY_VERGE,
  cliCommandSwitchesNodes,
  formatCliHuman,
  resolveCliClient,
  resolveCliNode,
} from "./dispatch";
import { CLI_COMMANDS, CLI_HELP_TEXT, parseCliArgv } from "./parse";
import { cliExitCode, errEnvelope, okEnvelope } from "./types";
import cliModeCases from "./cli-mode-cases.json";
// Vite ?raw：不依赖 @types/node 即可读 Rust 源码做同步断言
import libRs from "../../../src-tauri/src/lib.rs?raw";

describe("parseCliArgv", () => {
  it("无参数 → 非 cliMode + help", () => {
    const p = parseCliArgv(["/app/clash-assist"]);
    expect(p.cliMode).toBe(false);
    expect(p.command).toBe("help");
    expect(p.parseError).toBeNull();
  });

  it("--cli help", () => {
    const p = parseCliArgv(["clash-assist", "--cli", "help"]);
    expect(p.cliMode).toBe(true);
    expect(p.command).toBe("help");
    expect(p.parseError).toBeNull();
  });

  it("discover --client verge --json", () => {
    const p = parseCliArgv([
      "clash-assist",
      "--cli",
      "discover",
      "--client",
      "verge",
      "--json",
    ]);
    expect(p.command).toBe("discover");
    expect(p.clientId).toBe("verge");
    expect(p.clientGiven).toBe(true);
    expect(p.json).toBe(true);
    expect(p.mock).toBe(false);
    expect(p.parseError).toBeNull();
  });

  it("check current / all / node / 简写", () => {
    expect(parseCliArgv(["bin", "--cli", "check"]).checkTarget).toEqual({
      kind: "current",
    });
    expect(
      parseCliArgv(["bin", "--cli", "check", "current"]).checkTarget,
    ).toEqual({ kind: "current" });
    expect(parseCliArgv(["bin", "--cli", "check", "all"]).checkTarget).toEqual({
      kind: "all",
    });
    expect(
      parseCliArgv(["bin", "--cli", "check", "node", "香港", "01"]).checkTarget,
    ).toEqual({ kind: "node", name: "香港 01" });
    expect(
      parseCliArgv(["bin", "--cli", "check", "新加坡"]).checkTarget,
    ).toEqual({ kind: "node", name: "新加坡" });
  });

  it("--client=verge 与 -c 短选项", () => {
    expect(parseCliArgv(["bin", "--cli", "env", "--client=verge"]).clientId).toBe(
      "verge",
    );
    const p = parseCliArgv(["bin", "--cli", "env", "-c", "flclash"]);
    // 解析层只记原值；闸门在 resolveCliClient（见下）
    expect(p.clientId).toBe("flclash");
    expect(p.clientGiven).toBe(true);
    expect(p.parseError).toBeNull();
  });

  it("--mock --no-json", () => {
    const p = parseCliArgv(["bin", "--cli", "gate", "--mock", "--no-json"]);
    expect(p.mock).toBe(true);
    expect(p.json).toBe(false);
    expect(p.clientGiven).toBe(false);
  });

  it("--help 覆盖子命令", () => {
    const p = parseCliArgv(["bin", "--cli", "check", "all", "--help"]);
    expect(p.command).toBe("help");
    expect(p.checkTarget).toBeUndefined();
  });

  it("-- 之后都是位置参数（节点名以 - 开头）", () => {
    const p = parseCliArgv(["bin", "--cli", "check", "node", "--", "-奇怪", "名"]);
    expect(p.parseError).toBeNull();
    expect(p.checkTarget).toEqual({ kind: "node", name: "-奇怪 名" });
  });

  describe("严格报错（不静默变成 help）", () => {
    it("未知子命令 → unknown_command", () => {
      const p = parseCliArgv(["bin", "--cli", "frobnicate"]);
      expect(p.cliMode).toBe(true);
      expect(p.commandToken).toBe("frobnicate");
      expect(p.parseError?.code).toBe("unknown_command");
      expect(p.parseError?.message).toContain("frobnicate");
    });

    it("非 --cli 且首词不是子命令 → 交给 GUI，不报错", () => {
      const p = parseCliArgv(["bin", "frobnicate"]);
      expect(p.cliMode).toBe(false);
      expect(p.parseError).toBeNull();
    });

    it("未知选项 → unknown_option", () => {
      const p = parseCliArgv(["bin", "--cli", "discover", "--verbose"]);
      expect(p.parseError).toEqual({
        code: "unknown_option",
        message: expect.stringContaining("--verbose"),
      });
      expect(parseCliArgv(["bin", "--cli", "gate", "-x"]).parseError?.code).toBe(
        "unknown_option",
      );
    });

    it("只报第一处错误", () => {
      const p = parseCliArgv(["bin", "--cli", "--bogus", "frobnicate"]);
      expect(p.parseError?.code).toBe("unknown_option");
    });

    it("--client 缺值 → missing_option_value", () => {
      for (const argv of [
        ["bin", "--cli", "discover", "--client"],
        ["bin", "--cli", "discover", "--client", "--mock"],
        ["bin", "--cli", "discover", "-c", ""],
        ["bin", "--cli", "discover", "--client="],
      ]) {
        const p = parseCliArgv(argv);
        expect(p.parseError?.code, argv.join(" ")).toBe("missing_option_value");
        expect(p.clientId).toBeNull();
      }
      // 缺值时不吞后面的 flag
      expect(
        parseCliArgv(["bin", "--cli", "discover", "--client", "--mock"]).mock,
      ).toBe(true);
    });

    it("多余参数 → unexpected_argument", () => {
      for (const argv of [
        ["bin", "--cli", "discover", "extra"],
        ["bin", "--cli", "gate", "a", "b"],
        ["bin", "--cli", "env", "x"],
        ["bin", "--cli", "help", "check"],
        ["bin", "--cli", "check", "all", "extra"],
        ["bin", "--cli", "check", "current", "extra"],
      ]) {
        expect(parseCliArgv(argv).parseError?.code, argv.join(" ")).toBe(
          "unexpected_argument",
        );
      }
    });

    it("check node 不带名字 → node_name_required", () => {
      expect(
        parseCliArgv(["bin", "--cli", "check", "node"]).parseError?.code,
      ).toBe("node_name_required");
      expect(
        parseCliArgv(["bin", "--cli", "check", "node", "  "]).parseError?.code,
      ).toBe("node_name_required");
    });
  });
});

describe("TS / Rust 同步", () => {
  it("子命令全集与 Rust CLI_VERBS 一致", () => {
    const m = libRs.match(/pub const CLI_VERBS: &\[&str\] = &\[([^\]]*)\];/);
    expect(m, "lib.rs 里找不到 CLI_VERBS").toBeTruthy();
    const rustVerbs = [...m![1]!.matchAll(/"([^"]+)"/g)].map((x) => x[1]);
    expect(rustVerbs).toEqual([...CLI_COMMANDS]);
  });

  it("cliMode 判定与 Rust is_cli_mode_args 共用用例", () => {
    expect(cliModeCases.length).toBeGreaterThan(10);
    for (const c of cliModeCases) {
      expect(parseCliArgv(c.argv).cliMode, JSON.stringify(c.argv)).toBe(
        c.cliMode,
      );
    }
  });

  it("Rust 侧注册了中断轮询命令", () => {
    expect(libRs).toMatch(/fn cli_abort_requested\(\) -> bool/);
    expect(libRs).toMatch(/cli_abort_requested/);
  });

  it("Rust 侧注册了 serve 命令", () => {
    expect(libRs).toMatch(/fn cli_serve_start/);
    expect(libRs).toMatch(/fn cli_serve_poll/);
    expect(libRs).toMatch(/fn cli_serve_respond/);
    expect(libRs).toMatch(/fn cli_serve_stop/);
    expect(libRs).toContain('cli_serve');
  });
});

describe("resolveCliClient（与 GUI normalizeClientId 对齐）", () => {
  const base = { clientId: null, clientGiven: false, mock: false };

  it("未给 --client 且非 mock → client_required", () => {
    const r = resolveCliClient(base);
    expect(r.ok).toBe(false);
    if (!r.ok) {
      expect(r.error.code).toBe("client_required");
      expect(r.error.message).toContain("当前仅支持 Clash Verge");
      expect(r.error.message).not.toMatch(/flclash|clashx|mihomo_party|nyanpasu/i);
    }
  });

  it("--mock 且未给 --client → verge", () => {
    expect(resolveCliClient({ ...base, mock: true })).toEqual({
      ok: true,
      clientId: "verge",
    });
  });

  it("verge 通过", () => {
    expect(
      resolveCliClient({ ...base, clientId: "verge", clientGiven: true }),
    ).toEqual({ ok: true, clientId: "verge" });
  });

  it("flclash 被拒，--mock 也不回落", () => {
    for (const mock of [false, true]) {
      const r = resolveCliClient({ clientId: "flclash", clientGiven: true, mock });
      expect(r.ok).toBe(false);
      if (!r.ok) {
        expect(r.error.code).toBe("client_unsupported");
        expect(r.error.message).toContain("当前仅支持 Clash Verge");
        expect(r.error.message).toContain("FlClash 即将支持");
      }
    }
  });

  it("其它已知/未知值被拒（含大小写变体），--mock 也不回落", () => {
    for (const v of ["clashx_meta", "mihomo_party", "nyanpasu", "Verge", "foo", "mihomo"]) {
      for (const mock of [false, true]) {
        const r = resolveCliClient({ clientId: v, clientGiven: true, mock });
        expect(r.ok, `${v} mock=${mock}`).toBe(false);
        if (!r.ok) {
          expect(r.error.code).toBe("client_unsupported");
          expect(r.error.message).toContain(CLI_ONLY_VERGE);
        }
      }
    }
  });

  it("给了 --client 但值为空 → client_required", () => {
    const r = resolveCliClient({ clientId: null, clientGiven: true, mock: true });
    expect(r.ok).toBe(false);
    if (!r.ok) expect(r.error.code).toBe("client_required");
  });

  it("与 normalizeClientId 判定完全一致", () => {
    for (const v of ["verge", "flclash", "clashx_meta", "mihomo_party", "nyanpasu", "x", ""]) {
      const r = resolveCliClient({ clientId: v || null, clientGiven: true, mock: false });
      expect(r.ok, v).toBe(normalizeClientId(v) !== null);
    }
  });
});

describe("resolveCliNode", () => {
  const node = (name: string): ProxyNode => ({
    name,
    type: "ss",
    region: "未知",
    raw: { name, type: "ss" },
  });
  const nodes = [node("香港 01"), node("香港 02"), node("日本 01"), node("新加坡")];

  it("精确匹配优先", () => {
    const r = resolveCliNode([...nodes, node("香港")], "香港");
    expect(r.ok && r.node.name).toBe("香港");
  });

  it("唯一包含匹配", () => {
    const r = resolveCliNode(nodes, "日本");
    expect(r.ok && r.node.name).toBe("日本 01");
  });

  it("多个候选 → node_ambiguous", () => {
    const r = resolveCliNode(nodes, "香港");
    expect(r.ok).toBe(false);
    if (!r.ok) {
      expect(r.error.code).toBe("node_ambiguous");
      expect(r.error.message).toContain("香港 01");
    }
  });

  it("找不到 → node_not_found（不拿假节点去切换）", () => {
    const r = resolveCliNode(nodes, "美国");
    expect(r.ok).toBe(false);
    if (!r.ok) expect(r.error.code).toBe("node_not_found");
  });

  it("列表为空 → nodes_unavailable，带原因", () => {
    const r = resolveCliNode([], "香港", "读取失败");
    expect(r.ok).toBe(false);
    if (!r.ok) {
      expect(r.error.code).toBe("nodes_unavailable");
      expect(r.error.message).toContain("读取失败");
    }
  });
});

describe("cliCommandSwitchesNodes", () => {
  it("只有 check node / all 需要先切回再退出", () => {
    const sw = (argv: string[]) => cliCommandSwitchesNodes(parseCliArgv(["bin", "--cli", ...argv]));
    expect(sw(["check", "all"])).toBe(true);
    expect(sw(["check", "node", "x"])).toBe(true);
    expect(sw(["check", "x"])).toBe(true);
    expect(sw(["check", "current"])).toBe(false);
    expect(sw(["check"])).toBe(false);
    expect(sw(["discover"])).toBe(false);
    expect(sw(["gate"])).toBe(false);
    expect(sw(["env"])).toBe(false);
    expect(sw(["frobnicate"])).toBe(false);
  });
});

describe("CliEnvelope", () => {
  it("ok / err 形状稳定", () => {
    const ok = okEnvelope("discover", { x: 1 }, "0.1.16");
    expect(ok.ok).toBe(true);
    expect(ok.version).toBe("0.1.16");
    expect(ok.data).toEqual({ x: 1 });
    expect(ok.error).toBeUndefined();
    const err = errEnvelope("gate", "client_required", "缺 client", "0.1.16");
    expect(err.ok).toBe(false);
    expect(err.error?.code).toBe("client_required");
    expect("data" in err).toBe(false);
    const withData = errEnvelope("check", "aborted", "中断", "0.1.16", { n: 1 });
    expect(withData.data).toEqual({ n: 1 });
  });

  it("退出码：ok → 0，否则 1", () => {
    expect(cliExitCode({ ok: true })).toBe(0);
    expect(cliExitCode({ ok: false })).toBe(1);
  });
});

describe("帮助与文案", () => {
  it("只推广 verge；FlClash 标为即将支持", () => {
    expect(CLI_HELP_TEXT).toContain("0.2.0");
    expect(CLI_HELP_TEXT).toContain("仅支持 verge");
    expect(CLI_HELP_TEXT).toContain("FlClash 即将支持");
    expect(CLI_HELP_TEXT).toContain("serve");
    expect(CLI_HELP_TEXT).toContain("17890");
    expect(CLI_HELP_TEXT).not.toMatch(/clashx_meta|mihomo_party|nyanpasu/);
    expect(CLI_HELP_TEXT).not.toMatch(/\|\s*flclash/);
  });
});

describe("formatCliHuman（--no-json）", () => {
  it("help 打印纯文本", () => {
    expect(formatCliHuman(okEnvelope("help", { text: "HELP" }, "0.1.16"))).toBe(
      "HELP",
    );
  });

  it("失败：首行 ✗ + code + message，次行建议（hint）", () => {
    const out = formatCliHuman(
      errEnvelope("frobnicate", "unknown_command", "未知命令：frobnicate", "0.2.0"),
    );
    const lines = out.split("\n");
    expect(lines[0]).toBe("✗ frobnicate 失败 [unknown_command] 未知命令：frobnicate");
    expect(lines[1]).toMatch(/^  建议：.*help --json/);
  });

  it("discover 摘要（含截断提示），不是 JSON", () => {
    const out = formatCliHuman(
      okEnvelope(
        "discover",
        {
          clientId: "verge",
          status: "connected",
          message: "ok",
          currentProxy: "香港 01",
          usingMock: false,
          config: null,
          nodeCount: 80,
          nodesTruncated: true,
          nodesError: null,
          nodes: [],
        },
        "0.1.16",
      ),
    );
    expect(out.startsWith("✓ discover 完成")).toBe(true);
    expect(out).toContain("Clash Verge");
    expect(out).toContain("80 个");
    expect(out).toContain("前 50 个");
    expect(out).not.toContain("{");
  });

  it("check all 失败也打印已测结果与切回报错", () => {
    const out = formatCliHuman(
      errEnvelope("check", "restore_failed", "没能切回原先节点「A」。", "0.1.16", {
        target: "all",
        clientId: "verge",
        count: 1,
        scores: [{ nodeName: "B", stars: 4, totalScore: 80, blurb: "不错" }],
        hint: null,
        restoreError: "没能切回原先节点「A」。",
        aborted: false,
        nodesError: null,
        gate: { ok: true, message: "ok" },
      }),
    );
    expect(out).toContain("✗ check all 失败 [restore_failed]");
    expect(out).toContain("★4");
    expect(out).toContain("B — 不错");
    expect(out).toContain("⚠ 没能切回原先节点「A」。");
  });

  it("gate_failed 的 check 摘要显示门槛原因", () => {
    const out = formatCliHuman(
      errEnvelope("check", "gate_failed", "连不上", "0.1.16", {
        target: "node",
        clientId: "verge",
        gate: { ok: false, message: "连不上" },
      }),
    );
    expect(out).toContain("✗ check node 失败 [gate_failed] 连不上");
    expect(out).toContain("门槛：未通过 — 连不上");
  });
});
