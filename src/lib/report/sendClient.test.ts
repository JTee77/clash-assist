import { describe, expect, it } from "vitest";
import { buildReport, type ReportPhrases } from "./diagnostic";
import { assembleInput } from "./diagnostic";
import {
  FEEDBACK_URL,
  interpretFeedback,
  MAX_SENDS_PER_WINDOW,
  postJsonFollow,
  RATE_LIMITED,
  SEND_WINDOW_MS,
  sendDiagnostic,
  type KeyValueStore,
  type PostOnce,
  type PostOnceResult,
} from "./sendClient";

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
  levelLabel: (level) => (level === "pass" ? "通过" : "失败"),
  moreNodes: (n) => `其余 ${n} 个未列出`,
};

function memoryStore(): KeyValueStore & { raw: Map<string, string> } {
  const raw = new Map<string, string>();
  return {
    raw,
    get: (k) => raw.get(k) ?? null,
    set: (k, v) => {
      raw.set(k, v);
    },
  };
}

const REPORT = "version 0.3.2\nrestore_failed: 超时未响应";

describe("postJsonFollow", () => {
  it("re-POSTs the original JSON after a 302 instead of dropping the body", async () => {
    const calls: { url: string; body: string }[] = [];
    const postOnce: PostOnce = async (url, body) => {
      calls.push({ url, body });
      if (calls.length === 1) {
        return {
          status: 302,
          location: "https://script.googleusercontent.com/macros/echo?user_content_key=abc",
          body: "",
        };
      }
      return { status: 200, location: null, body: JSON.stringify({ ok: true }) };
    };
    const json = JSON.stringify({ version: "0.3.2", os: "macos aarch64", client: "verge", report: REPORT });
    const res = await postJsonFollow(FEEDBACK_URL, json, postOnce);
    expect(res.status).toBe(200);
    expect(calls).toHaveLength(2);
    expect(calls[0]!.url).toBe(FEEDBACK_URL);
    expect(calls[1]!.url).toBe("https://script.googleusercontent.com/macros/echo?user_content_key=abc");
    expect(calls[1]!.body).toBe(json);
    expect(calls[1]!.body).toBe(calls[0]!.body);
    expect(JSON.parse(calls[1]!.body).report).toBe(REPORT);
  });

  it("resolves a relative Location and still POSTs the same body", async () => {
    const calls: { url: string; body: string }[] = [];
    const postOnce: PostOnce = async (url, body) => {
      calls.push({ url, body });
      if (calls.length === 1) return { status: 302, location: "/macros/echo?x=1", body: "" };
      return { status: 200, location: null, body: JSON.stringify({ ok: true }) };
    };
    await postJsonFollow("https://script.google.com/macros/s/ABC/exec", "{\"a\":1}", postOnce);
    expect(calls[1]!.url).toBe("https://script.google.com/macros/echo?x=1");
    expect(calls[1]!.body).toBe("{\"a\":1}");
  });
});

describe("interpretFeedback", () => {
  it("treats HTTP 200 with ok:false as failure", () => {
    expect(interpretFeedback(200, JSON.stringify({ ok: false, error: "nope" }))).toEqual({
      ok: false,
      error: "nope",
    });
    expect(interpretFeedback(200, JSON.stringify({ ok: true }))).toEqual({ ok: true });
    expect(interpretFeedback(500, JSON.stringify({ ok: true })).ok).toBe(false);
  });
});

describe("sendDiagnostic", () => {
  const input = {
    version: "0.3.2",
    os: "macos aarch64",
    client: "verge",
    report: REPORT,
  };

  function script(handler: (url: string) => PostOnceResult): {
    postOnce: PostOnce;
    calls: { url: string; body: string }[];
  } {
    const calls: { url: string; body: string }[] = [];
    const postOnce: PostOnce = async (url, body) => {
      calls.push({ url, body });
      return handler(url);
    };
    return { postOnce, calls };
  }

  it("sends the sanitized markdown and keeps it when the server returns ok:false", async () => {
    const built = buildReport(
      assembleInput({
        version: "0.3.2",
        os: "macos",
        arch: "aarch64",
        client: "verge",
        nodeNames: ["香港 IEPL 01"],
        tested: [],
        envCards: [],
        errors: [{ code: "restore_failed", message: "没能切回原先节点「香港 IEPL 01」。出口 8.8.8.8" }],
        restoreSucceeded: false,
        nodeLabel: (n) => `节点 ${n}`,
      }),
      phrases,
    );
    const { postOnce, calls } = script(() => ({
      status: 200,
      location: null,
      body: JSON.stringify({ ok: false, error: "busy" }),
    }));
    const store = memoryStore();
    const first = await sendDiagnostic(
      { version: built.version, os: `${built.os} ${built.arch}`, client: built.client, report: built.markdown },
      { postOnce, store, now: 1_000 },
    );
    expect(first.ok).toBe(false);
    expect(first.error).toBe("busy");
    expect(first.report).toBe(built.markdown);
    expect(built.markdown).not.toContain("香港 IEPL 01");
    expect(built.markdown).not.toContain("8.8.8.8");
    const sent = JSON.parse(calls[0]!.body) as { version: string; os: string; client: string; report: string; contact?: string };
    expect(sent).toEqual({
      version: "0.3.2",
      os: "macos aarch64",
      client: "verge",
      report: built.markdown,
    });
    expect(sent.contact).toBeUndefined();

    const second = await sendDiagnostic(
      { version: built.version, os: `${built.os} ${built.arch}`, client: built.client, report: first.report, contact: "me@example.com" },
      { postOnce, store, now: 2_000 },
    );
    expect(second.ok).toBe(false);
    expect(second.report).toBe(first.report);
    const retried = JSON.parse(calls[1]!.body) as { report: string; contact?: string };
    expect(retried.report).toBe(built.markdown);
    expect(retried.contact).toBe("me@example.com");
  });

  it("follows the redirect and only succeeds on ok:true", async () => {
    const { postOnce, calls } = script((url) =>
      url === FEEDBACK_URL
        ? { status: 302, location: "https://script.googleusercontent.com/macros/echo?x=1", body: "" }
        : { status: 200, location: null, body: JSON.stringify({ ok: true }) },
    );
    const result = await sendDiagnostic(input, { postOnce, store: memoryStore(), now: 1 });
    expect(result).toEqual({ ok: true, report: REPORT });
    expect(JSON.parse(calls[1]!.body).report).toBe(REPORT);
    expect(calls[1]!.body).toBe(calls[0]!.body);
    expect(calls[1]!.url).toContain("script.googleusercontent.com");
  });

  it("rate-limits further sends, persists the window, and still returns the report", async () => {
    const store = memoryStore();
    let posts = 0;
    const postOnce: PostOnce = async () => {
      posts += 1;
      return { status: 200, location: null, body: JSON.stringify({ ok: false, error: "nope" }) };
    };
    const now = 10_000;
    for (let i = 0; i < MAX_SENDS_PER_WINDOW; i++) {
      const result = await sendDiagnostic(input, { postOnce, store, now: now + i });
      expect(result.ok).toBe(false);
      expect(result.report).toBe(REPORT);
    }
    expect(posts).toBe(MAX_SENDS_PER_WINDOW);
    const blocked = await sendDiagnostic(input, { postOnce, store, now: now + 10 });
    expect(blocked.ok).toBe(false);
    expect(blocked.error).toBe(RATE_LIMITED);
    expect(blocked.report).toBe(REPORT);
    expect(blocked.retryAfterMs).toBeGreaterThan(0);
    expect(posts).toBe(MAX_SENDS_PER_WINDOW);

    const later = await sendDiagnostic(input, {
      postOnce,
      store,
      now: now + SEND_WINDOW_MS + 1,
    });
    expect(later.ok).toBe(false);
    expect(later.error).toBe("nope");
    expect(later.report).toBe(REPORT);
    expect(posts).toBe(MAX_SENDS_PER_WINDOW + 1);
  });

  it("keeps the report when the transport throws", async () => {
    const postOnce: PostOnce = async () => {
      throw new Error("network down at 1.2.3.4");
    };
    const result = await sendDiagnostic(input, { postOnce, store: memoryStore(), now: 1 });
    expect(result.ok).toBe(false);
    expect(result.report).toBe(REPORT);
    expect(result.error).not.toContain("1.2.3.4");
    expect(result.error).toContain("[ip]");
  });
});
