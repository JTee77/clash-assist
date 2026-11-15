import { describe, expect, it } from "vitest";
import { buildReport, type ReportPhrases } from "./diagnostic";
import { assembleInput } from "./diagnostic";
import {
  FEEDBACK_URL,
  interpretFeedback,
  isEchoResultUrl,
  MAX_SENDS_PER_WINDOW,
  postJsonFollow,
  RATE_LIMITED,
  SEND_WINDOW_MS,
  sendDiagnostic,
  type GetOnce,
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
  nodeLabel: (n) => `节点 ${n}`,
  levelLabel: (level) => (level === "pass" ? "通过" : "失败"),
};

const ECHO = "https://script.googleusercontent.com/macros/echo?user_content_key=abc&lib=xyz";
const DISCLAIMER = "非官方诊断结果，与 Clash 或任何机场、VPN 服务无关。";
const REPO = "https://github.com/JTee77/clash-assist";

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

type Hop = { method: "POST" | "GET"; url: string; body: string | null };

/**
 * 模拟官方表单：POST /exec 已收下报告并 302/303 到 echo。
 * echo 再被 POST 时返回 405（Allow: GET, HEAD）；GET 才返回调用方给的正文。
 * 不访问生产环境。
 */
function appsScript(status: 302 | 303 | 307 | 301, echoBody: string, echo = ECHO) {
  const calls: Hop[] = [];
  const postOnce: PostOnce = async (url, body) => {
    calls.push({ method: "POST", url, body });
    if (isEchoResultUrl(url)) {
      return { status: 405, location: null, body: "Allow: HEAD, GET" };
    }
    return { status, location: echo, body: "" };
  };
  const getOnce: GetOnce = async (url) => {
    calls.push({ method: "GET", url, body: null });
    return { status: 200, location: null, body: echoBody };
  };
  return { calls, postOnce, getOnce };
}

describe("postJsonFollow", () => {
  it.each([302, 303, 301] as const)(
    "follows %s to the echo URL with GET and does not re-POST the JSON",
    async (status) => {
      const json = JSON.stringify({
        version: "0.3.2",
        os: "macos aarch64",
        client: "verge",
        report: REPORT,
      });
      const { calls, postOnce, getOnce } = appsScript(status, JSON.stringify({ ok: true }));
      const res = await postJsonFollow(FEEDBACK_URL, json, postOnce, getOnce);
      expect(res).toEqual({ status: 200, location: null, body: JSON.stringify({ ok: true }) });
      expect(calls.map((c) => c.method)).toEqual(["POST", "GET"]);
      expect(calls[0]).toEqual({ method: "POST", url: FEEDBACK_URL, body: json });
      expect(calls[1]).toEqual({ method: "GET", url: ECHO, body: null });
      expect(calls.filter((c) => c.method === "POST" && isEchoResultUrl(c.url))).toEqual([]);
      expect(JSON.parse(calls[0]!.body!).report).toBe(REPORT);
    },
  );

  it("GETs a relative Location instead of posting the body again", async () => {
    const calls: Hop[] = [];
    const postOnce: PostOnce = async (url, body) => {
      calls.push({ method: "POST", url, body });
      return { status: 302, location: "/macros/echo?x=1", body: "" };
    };
    const getOnce: GetOnce = async (url) => {
      calls.push({ method: "GET", url, body: null });
      return { status: 200, location: null, body: JSON.stringify({ ok: true }) };
    };
    await postJsonFollow("https://script.google.com/macros/s/ABC/exec", "{\"a\":1}", postOnce, getOnce);
    expect(calls[1]).toEqual({
      method: "GET",
      url: "https://script.google.com/macros/echo?x=1",
      body: null,
    });
    expect(calls.filter((c) => c.method === "POST")).toHaveLength(1);
  });

  it("keeps POST for 307 when the next host is not the echo URL", async () => {
    const calls: Hop[] = [];
    const next = "https://script.google.com/macros/s/NEXT/exec";
    const postOnce: PostOnce = async (url, body) => {
      calls.push({ method: "POST", url, body });
      if (calls.length === 1) return { status: 307, location: next, body: "" };
      return { status: 200, location: null, body: JSON.stringify({ ok: true }) };
    };
    const getOnce: GetOnce = async (url) => {
      calls.push({ method: "GET", url, body: null });
      return { status: 500, location: null, body: "get should not run" };
    };
    const res = await postJsonFollow(FEEDBACK_URL, "{\"a\":1}", postOnce, getOnce);
    expect(res.status).toBe(200);
    expect(calls.map((c) => [c.method, c.url, c.body])).toEqual([
      ["POST", FEEDBACK_URL, "{\"a\":1}"],
      ["POST", next, "{\"a\":1}"],
    ]);
  });

  it("still GETs when a 307 points at the echo URL", async () => {
    const { calls, postOnce, getOnce } = appsScript(307, JSON.stringify({ ok: true }));
    const res = await postJsonFollow(FEEDBACK_URL, "{\"a\":1}", postOnce, getOnce);
    expect(res.status).toBe(200);
    expect(calls.map((c) => c.method)).toEqual(["POST", "GET"]);
    expect(isEchoResultUrl(calls[1]!.url)).toBe(true);
    expect(calls[1]!.body).toBeNull();
  });
});

describe("interpretFeedback", () => {
  it("treats HTTP 200 with ok:true as success and ok:false as failure", () => {
    expect(interpretFeedback(200, JSON.stringify({ ok: false, error: "nope" }))).toEqual({
      ok: false,
      error: "nope",
    });
    expect(interpretFeedback(200, JSON.stringify({ ok: true }))).toEqual({ ok: true });
    expect(interpretFeedback(500, JSON.stringify({ ok: true })).ok).toBe(false);
    expect(interpretFeedback(405, "Allow: HEAD, GET")).toEqual({ ok: false, error: "HTTP 405" });
  });
});

describe("sendDiagnostic", () => {
  const input = {
    version: "0.3.2",
    os: "macos aarch64",
    client: "verge",
    report: REPORT,
  };

  function script(handler: (url: string, method: "POST" | "GET") => PostOnceResult): {
    postOnce: PostOnce;
    getOnce: GetOnce;
    calls: Hop[];
  } {
    const calls: Hop[] = [];
    const postOnce: PostOnce = async (url, body) => {
      calls.push({ method: "POST", url, body });
      return handler(url, "POST");
    };
    const getOnce: GetOnce = async (url) => {
      calls.push({ method: "GET", url, body: null });
      return handler(url, "GET");
    };
    return { postOnce, getOnce, calls };
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
    const { postOnce, getOnce, calls } = script(() => ({
      status: 200,
      location: null,
      body: JSON.stringify({ ok: false, error: "busy" }),
    }));
    const store = memoryStore();
    const first = await sendDiagnostic(
      { version: built.version, os: `${built.os} ${built.arch}`, client: built.client, report: built.markdown },
      { postOnce, getOnce, store, now: 1_000 },
    );
    expect(first.ok).toBe(false);
    expect(first.error).toBe("busy");
    expect(first.report).toBe(built.markdown);
    expect(built.markdown).not.toContain("香港 IEPL 01");
    expect(built.markdown).not.toContain("8.8.8.8");
    expect(built.markdown).not.toContain(DISCLAIMER);
    expect(built.markdown).not.toContain(REPO);
    const sent = JSON.parse(calls[0]!.body!) as {
      version: string;
      os: string;
      client: string;
      report: string;
      contact?: string;
    };
    expect(sent).toEqual({
      version: "0.3.2",
      os: "macos aarch64",
      client: "verge",
      report: built.markdown,
    });
    expect(sent.contact).toBeUndefined();

    const second = await sendDiagnostic(
      {
        version: built.version,
        os: `${built.os} ${built.arch}`,
        client: built.client,
        report: first.report,
        contact: "me@example.com",
      },
      { postOnce, getOnce, store, now: 2_000 },
    );
    expect(second.ok).toBe(false);
    expect(second.report).toBe(first.report);
    const retried = JSON.parse(calls[1]!.body!) as { report: string; contact?: string };
    expect(retried.report).toBe(built.markdown);
    expect(retried.contact).toBe("me@example.com");
  });

  it("reports success after POST /exec and GET echo, not HTTP 405", async () => {
    expect(FEEDBACK_URL).toBe(
      "https://script.google.com/macros/s/AKfycbypPAXzah4RI1px9Wybr9maQI0pJUqhIYuPFeNkJjmy1waiLS9rOrBudcfpysBAqs10rA/exec",
    );
    const { calls, postOnce, getOnce } = appsScript(302, JSON.stringify({ ok: true }));
    const result = await sendDiagnostic(input, { postOnce, getOnce, store: memoryStore(), now: 1 });
    expect(result).toEqual({ ok: true, report: REPORT });
    expect(result.error).toBeUndefined();
    expect(calls.map((c) => c.method)).toEqual(["POST", "GET"]);
    expect(calls[1]!.url).toContain("script.googleusercontent.com/macros/echo");
    expect(calls[1]!.body).toBeNull();
    expect(JSON.parse(calls[0]!.body!).report).toBe(REPORT);
  });

  it("treats HTTP 200 {ok:false} from the echo GET as failure and keeps the report", async () => {
    const { postOnce, getOnce } = appsScript(302, JSON.stringify({ ok: false, error: "nope" }));
    const result = await sendDiagnostic(input, { postOnce, getOnce, store: memoryStore(), now: 1 });
    expect(result.ok).toBe(false);
    expect(result.error).toBe("nope");
    expect(result.report).toBe(REPORT);
    const again = await sendDiagnostic(input, { postOnce, getOnce, store: memoryStore(), now: 2 });
    expect(again.report).toBe(REPORT);
    expect(again.ok).toBe(false);
  });

  it("rate-limits further sends, persists the window, and still returns the report", async () => {
    const store = memoryStore();
    let posts = 0;
    const postOnce: PostOnce = async () => {
      posts += 1;
      return { status: 200, location: null, body: JSON.stringify({ ok: false, error: "nope" }) };
    };
    const getOnce: GetOnce = async () => {
      throw new Error("no redirect");
    };
    const now = 10_000;
    for (let i = 0; i < MAX_SENDS_PER_WINDOW; i++) {
      const result = await sendDiagnostic(input, { postOnce, getOnce, store, now: now + i });
      expect(result.ok).toBe(false);
      expect(result.report).toBe(REPORT);
    }
    expect(posts).toBe(MAX_SENDS_PER_WINDOW);
    const blocked = await sendDiagnostic(input, { postOnce, getOnce, store, now: now + 10 });
    expect(blocked.ok).toBe(false);
    expect(blocked.error).toBe(RATE_LIMITED);
    expect(blocked.report).toBe(REPORT);
    expect(blocked.retryAfterMs).toBeGreaterThan(0);
    expect(posts).toBe(MAX_SENDS_PER_WINDOW);

    const later = await sendDiagnostic(input, {
      postOnce,
      getOnce,
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
    const getOnce: GetOnce = async () => {
      throw new Error("no redirect");
    };
    const result = await sendDiagnostic(input, { postOnce, getOnce, store: memoryStore(), now: 1 });
    expect(result.ok).toBe(false);
    expect(result.report).toBe(REPORT);
    expect(result.error).not.toContain("1.2.3.4");
    expect(result.error).toContain("[ip]");
  });
});
