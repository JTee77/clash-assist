import { describe, expect, it } from "vitest";
import {
  assembleInput,
  buildReport,
  REPO_URL,
  type ReportPhrases,
} from "./diagnostic";
import { cleanContact, sanitizeText } from "./sanitize";

const phrases: ReportPhrases = {
  docTitle: "诊断报告",
  version: "版本",
  os: "系统",
  client: "客户端",
  nodeCount: "节点数",
  restore: "切回原节点",
  restoreSucceeded: "已切回原节点",
  restoreFailed: "未能切回原节点",
  restoreNotRun: "未进行切回",
  checks: "检查项",
  errors: "错误",
  disclaimer: "非官方诊断结果，与 Clash 或任何机场、VPN 服务无关。",
  nodeLabel: (n) => `节点 ${n}`,
  levelLabel: (level) =>
    level === "pass" ? "通过" : level === "fail" ? "失败" : level === "warn" ? "警告" : "未知",
  moreNodes: (n) => `其余 ${n} 个未列出`,
};

const SUB = "https://sub.example.com/api/v1/client/subscribe?token=abcdef0123456789abcdef0123456789";
const SECRET = "0123456789abcdef0123456789abcdef";
const NODE = "香港 IEPL 01";
const IP = "8.8.8.8";
const V6 = "2001:db8::1";
const HOST = "api.ipify.org";
const PATH = "/Users/alice/Library/Application Support/clash/config.yaml";
const WIN_PATH = "C:\\Users\\alice\\AppData\\Roaming\\clash\\config.yaml";

describe("sanitizeText", () => {
  it("strips subscription URLs, tokens, IPs, hostnames, usernames, and home paths", () => {
    const raw = [
      `订阅 ${SUB}`,
      "vmess://eyJ2IjoiMiIsInBzIjoiTm9kZSJ9",
      `secret: ${SECRET}`,
      "Bearer abcdef0123456789abcdef0123456789",
      `username: alice`,
      `alice@example.com`,
      `出口 ${IP} 与 ${V6} 以及 ::ffff:192.0.2.1`,
      `探测 ${HOST}`,
      PATH,
      WIN_PATH,
      "/home/bob/.config/clash/config.yaml",
    ].join("\n");
    const out = sanitizeText(raw);
    for (const leak of [SUB, "vmess://", SECRET, "abcdef0123456789abcdef0123456789", "alice", "bob", IP, V6, "192.0.2.1", HOST, "example.com", "/Users/", "\\Users\\", "/home/"]) {
      expect(out, leak).not.toContain(leak);
    }
    expect(out).toContain("[url]");
    expect(out).toContain("[secret]");
    expect(out).toContain("[ip]");
    expect(out).toContain("[host]");
    expect(out).toContain("[path]");
    expect(out).toContain("[user]");
  });

  it("keeps version-like numbers and ordinary check wording", () => {
    const out = sanitizeText("版本 0.3.2，延迟 1.2.3 ms，连通性通过");
    expect(out).toContain("0.3.2");
    expect(out).toContain("1.2.3");
    expect(out).toContain("连通性通过");
    expect(out).not.toContain("[ip]");
  });

  it("replaces node names with the caller-supplied index and leaves short letters alone", () => {
    const out = sanitizeText(`没能切回原先节点「${NODE}」，也提到了「A」。`, [
      { name: NODE, replacement: "节点 1" },
      { name: "A", replacement: "节点 2" },
    ]);
    expect(out).toContain("节点 1");
    expect(out).toContain("节点 2");
    expect(out).not.toContain(NODE);
    expect(out).not.toMatch(/「A」/);
  });
});

describe("buildReport", () => {
  const dirty = () =>
    assembleInput({
      version: "0.3.2",
      os: "macos",
      arch: "aarch64",
      client: "verge",
      nodeNames: [NODE, "https://evil.example/sub?token=abcdef0123456789abcdef0123456789", "未测节点"],
      tested: [
        {
          name: NODE,
          cards: [
            { name: "连通性", level: "pass", message: "通" },
            { name: "出口 IP", level: "fail", message: `出口 ${IP} via ${HOST}` },
          ],
        },
        {
          name: "https://evil.example/sub?token=abcdef0123456789abcdef0123456789",
          cards: [{ name: "连通性", level: "warn", message: `配置在 ${PATH}` }],
        },
      ],
      envCards: [
        { name: "DNS 解析器", level: "fail", message: `解析到 ${IP}` },
        { name: "WebRTC", level: "unknown", message: "尚未检测" },
      ],
      errors: [
        { code: "restore_failed", message: `没能切回原先节点「${NODE}」。secret: ${SECRET}` },
      ],
      restoreSucceeded: false,
      nodeLabel: (n) => `节点 ${n}`,
    });

  it("keeps version, OS, arch, client, error code, check names, pass/fail, node count, and restore outcome", () => {
    const report = buildReport(dirty(), phrases);
    expect(report.version).toBe("0.3.2");
    expect(report.os).toBe("macos");
    expect(report.arch).toBe("aarch64");
    expect(report.client).toBe("verge");
    expect(report.nodeCount).toBe(3);
    expect(report.restore).toBe("failed");
    expect(report.errors.map((e) => e.code)).toEqual(["restore_failed"]);
    expect(report.checks.map((c) => [c.nodeIndex ?? 0, c.name, c.level])).toEqual([
      [0, "DNS 解析器", "fail"],
      [1, "连通性", "pass"],
      [1, "出口 IP", "fail"],
      [2, "连通性", "warn"],
    ]);
    expect(report.markdown).toContain("0.3.2");
    expect(report.markdown).toContain("macos aarch64");
    expect(report.markdown).toContain("verge");
    expect(report.markdown).toContain("restore_failed");
    expect(report.markdown).toContain("DNS 解析器");
    expect(report.markdown).toContain("连通性");
    expect(report.markdown).toContain("通过");
    expect(report.markdown).toContain("失败");
    expect(report.markdown).toContain("节点数: 3");
    expect(report.markdown).toContain("未能切回原节点");
  });

  it("does not leak secrets, and the share text uses indexes plus the disclaimer and repo URL", () => {
    const report = buildReport(dirty(), phrases);
    const blob = `${report.markdown}\n${report.shareLines.join("\n")}`;
    for (const leak of [NODE, "未测节点", SUB, SECRET, IP, HOST, "alice", "evil.example", "vmess://", "/Users/"]) {
      expect(blob, leak).not.toContain(leak);
    }
    expect(report.shareLines.some((l) => l.startsWith("节点 1:"))).toBe(true);
    expect(report.shareLines.some((l) => l.startsWith("节点 2:"))).toBe(true);
    expect(report.shareLines.some((l) => l.includes("出口 IP:") && l.includes(IP))).toBe(false);
    expect(report.shareLines).toContain(phrases.disclaimer);
    expect(report.shareLines[report.shareLines.length - 1]).toBe(REPO_URL);
    expect(report.markdown).toContain(phrases.disclaimer);
    expect(report.markdown).toContain(REPO_URL);
  });

  it("records a successful restore and drops placeholder checks", () => {
    const input = assembleInput({
      version: "0.3.2",
      os: "windows",
      arch: "x86_64",
      client: "flclash",
      nodeNames: ["东京 01"],
      tested: [],
      envCards: [{ name: "IPv6 泄漏", level: "pass", message: "未发现公网地址" }],
      errors: [],
      restoreSucceeded: true,
      nodeLabel: (n) => `节点 ${n}`,
    });
    const report = buildReport(input, phrases);
    expect(report.restore).toBe("succeeded");
    expect(report.client).toBe("flclash");
    expect(report.checks).toEqual([
      { name: "IPv6 泄漏", level: "pass", message: "未发现公网地址", nodeIndex: undefined },
    ]);
    expect(report.markdown).toContain("已切回原节点");
  });
});

describe("cleanContact", () => {
  it("keeps an email and drops a pasted subscription URL", () => {
    expect(cleanContact("  alice@example.com  ")).toBe("alice@example.com");
    expect(cleanContact(`${SUB}`)).toBeUndefined();
    expect(cleanContact(`alice@example.com ${SUB}`)).toBe("alice@example.com [url]");
  });
});
