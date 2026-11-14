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

  it("removes spaced home paths, UNC, env homes, encoded paths, and labeled secrets", () => {
    const win = "C:\\Users\\Alice Smith\\AppData\\Roaming\\clash\\config.yaml";
    const mac = "/Users/Alice Smith/Library/Application Support/clash/config.yaml";
    const unc = "\\\\FILESERVER\\Users\\Alice Smith\\share\\config.yaml";
    const extended = "\\\\?\\C:\\Users\\Alice Smith\\AppData\\Local\\clash.log";
    const profile = "%USERPROFILE%\\AppData\\Roaming\\clash\\config.yaml";
    const home = "$HOME/Library/Application Support/clash/config.yaml";
    const encoded =
      "C:%5CUsers%5CAlice%20Smith%5CAppData%5CRoaming%5Cclash%5Cconfig.yaml";
    const encodedUnix = "%2FUsers%2FAlice%20Smith%2FLibrary%2FApplication%20Support%2Fclash";
    const uuid = "550e8400-e29b-41d4-a716-446655440000";
    const blob = "eyJ2IjoiMiIsInBzIjoiTm9kZSJ9";
    const userinfo = btoa("aes-256-gcm:s3cret@10.1.2.3:443");
    const raw = [
      `配置在 ${win}，请检查`,
      `以及 ${mac} 里`,
      `网络 ${unc} 结束`,
      "//FILESERVER/Users/Alice Smith/share/config.yaml",
      extended,
      profile,
      `${home} 完毕`,
      encoded,
      encodedUnix,
      `Cookie: session=abc123secret`,
      `uuid: ${uuid}`,
      "access_token=ya29.a0AfH6SMsecret",
      `vmess:${blob}`,
      blob,
      userinfo,
      "~/Library/Application Support/clash/prefs.plist",
      "客户端 Clash 与 DNS 正常，fileserver 只是句子",
      "版本 0.3.2，延迟 1.2.3 ms，连通性通过",
    ].join("\n");
    const out = sanitizeText(raw);
    for (const leak of [
      "Alice",
      "Smith",
      "AppData",
      "FILESERVER",
      "USERPROFILE",
      "Application",
      "Support",
      "abc123secret",
      uuid,
      "ya29",
      blob,
      "s3cret",
      "10.1.2.3",
      "vmess:",
      userinfo,
      "$HOME",
      "%USERPROFILE%",
    ]) {
      expect(out, leak).not.toContain(leak);
    }
    expect(out).toContain("[path]");
    expect(out).toContain("[secret]");
    expect(out).toContain("请检查");
    expect(out).toContain("里");
    expect(out).toContain("完毕");
    expect(out).toContain("fileserver");
    expect(out).toContain("Clash");
    expect(out).toContain("0.3.2");
    expect(out).toContain("1.2.3");
    expect(out).toContain("连通性通过");
    expect(sanitizeText("C:\\Users\\Alice Smith")).toBe("[path]");
    expect(sanitizeText("/Users/Alice Smith")).toBe("[path]");
    expect(sanitizeText("see /Users/Alice Smith and then more")).toBe("see [path] and then more");
    expect(sanitizeText("Cookie: a=b; c=d more")).toBe("[secret] more");
  });

  it("keeps real exit-IP, DNS, WebRTC, and IPv6 wording while removing the addresses", () => {
    const exit = "8.8.8.8 · US · 住宅(ISP)";
    const dns =
      "DNS 从 203.0.113.8（US）出去，与你的真实网络归属（CN）一致，而代理出口是 8.8.8.8 —— 疑似 DNS 泄漏真实地址";
    const webrtc =
      "WebRTC 暴露了 ≠ 代理出口的公网地址（203.0.113.10，a1b2c3d4.local）—— 可能泄漏真实 IP。";
    const ipv6 = "直连能拿到 IPv6（2001:db8::1），代理侧没有 —— IPv6 很可能绕过代理直连出去。";
    const mapped = "::ffff:192.0.2.10";
    const out = sanitizeText([exit, dns, webrtc, ipv6, mapped].join("\n"));
    for (const leak of ["8.8.8.8", "203.0.113.8", "203.0.113.10", "2001:db8::1", "192.0.2.10", "a1b2c3d4.local"]) {
      expect(out, leak).not.toContain(leak);
    }
    expect(out).toContain("[ip]");
    expect(out).toContain("[host]");
    expect(out).toContain("US");
    expect(out).toContain("CN");
    expect(out).toContain("住宅(ISP)");
    expect(out).toContain("疑似 DNS 泄漏真实地址");
    expect(out).toContain("可能泄漏真实 IP");
    expect(out).toContain("IPv6 很可能绕过代理直连出去");
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

  it("sanitizes realistic check copy and keeps explanations off the share card", () => {
    const win = "C:\\Users\\Alice Smith\\AppData\\Roaming\\clash\\config.yaml";
    const mac = "/Users/Alice Smith/Library/Application Support/clash/config.yaml";
    const uuid = "550e8400-e29b-41d4-a716-446655440000";
    const input = assembleInput({
      version: "0.3.2",
      os: "windows",
      arch: "x86_64",
      client: "verge",
      nodeNames: [NODE],
      tested: [
        {
          name: NODE,
          cards: [
            { name: "出口 IP", level: "pass", message: "8.8.8.8 · US · 住宅(ISP)" },
            {
              name: "连通性",
              level: "fail",
              message: `配置在 ${win} 与 ${mac}。Cookie: session=abc123secret uuid: ${uuid} access_token=ya29.a0AfH6SMsecret`,
            },
          ],
        },
      ],
      envCards: [
        {
          name: "DNS 解析器",
          level: "fail",
          message:
            "DNS 从 203.0.113.8（US）出去，与你的真实网络归属（CN）一致，而代理出口是 8.8.8.8 —— 疑似 DNS 泄漏真实地址",
        },
        {
          name: "WebRTC",
          level: "fail",
          message:
            "WebRTC 暴露了 ≠ 代理出口的公网地址（203.0.113.10）—— 可能泄漏真实 IP。",
        },
        {
          name: "IPv6 泄漏",
          level: "fail",
          message: "直连能拿到 IPv6（2001:db8::1），代理侧没有 —— IPv6 很可能绕过代理直连出去。",
        },
      ],
      errors: [{ code: "restore_failed", message: `没能切回原先节点「${NODE}」。` }],
      restoreSucceeded: false,
      nodeLabel: (n) => `节点 ${n}`,
    });
    const report = buildReport(input, phrases);
    const blob = `${report.markdown}\n${report.shareLines.join("\n")}`;
    for (const leak of [
      NODE,
      "Alice",
      "Smith",
      "8.8.8.8",
      "203.0.113.8",
      "203.0.113.10",
      "2001:db8::1",
      "abc123secret",
      uuid,
      "ya29",
      "AppData",
    ]) {
      expect(blob, leak).not.toContain(leak);
    }
    expect(report.markdown).toContain("[ip]");
    expect(report.markdown).toContain("[path]");
    expect(report.markdown).toContain("[secret]");
    expect(report.markdown).toContain("住宅(ISP)");
    expect(report.markdown).toContain("CN");
    expect(report.markdown).toContain("节点 1");
    expect(report.markdown).toContain("未能切回原节点");
    const share = report.shareLines.join("\n");
    expect(share).toContain("节点 1:");
    expect(share).toContain("DNS 解析器:");
    expect(share).not.toContain("[ip]");
    expect(share).not.toContain("[path]");
    expect(share).not.toContain("[secret]");
    expect(share).not.toContain("疑似 DNS");
    expect(share).not.toContain("可能泄漏");
    expect(share).not.toContain("住宅");
    expect(report.shareLines).toContain(phrases.disclaimer);
    expect(report.shareLines[report.shareLines.length - 1]).toBe(REPO_URL);

    const english = buildReport(input, { ...phrases, nodeLabel: (n) => `Node ${n}` });
    expect(english.shareLines.some((l) => l.startsWith("Node 1:"))).toBe(true);
    expect(english.markdown).not.toContain(NODE);
    expect(english.shareLines.join("\n")).not.toContain("可能泄漏");
  });
});

describe("cleanContact", () => {
  it("keeps an email and drops a pasted subscription URL", () => {
    expect(cleanContact("  alice@example.com  ")).toBe("alice@example.com");
    expect(cleanContact(`${SUB}`)).toBeUndefined();
    expect(cleanContact(`alice@example.com ${SUB}`)).toBe("alice@example.com [url]");
    expect(cleanContact("uuid: 550e8400-e29b-41d4-a716-446655440000")).toBeUndefined();
    expect(cleanContact("Cookie: session=abc123secret")).toBeUndefined();
    expect(cleanContact("alice@example.com Cookie: session=abc123secret")).toBe(
      "alice@example.com [secret]",
    );
  });
});
