/**
 * 错误报告发送客户端。
 *
 * 预览由界面持有；这里失败时原样把 report 交回去，调用方不要清掉预览。
 * Apps Script 的 exec 地址会 302 到 script.googleusercontent.com。
 * 浏览器和 reqwest 的默认策略都会把这次 302 的 POST 改成 GET 并丢掉正文，
 * 所以跳转由这里自己跟，每一次都用原来的 JSON 再 POST 一次。
 * 真正发出那一跳的传输层必须关闭自动重定向（见 Rust `feedback::exchange_once`）。
 */

import { cleanContact, sanitizeText } from "./sanitize";

export const FEEDBACK_URL =
  "https://script.google.com/macros/s/AKfycbypPAXzah4RI1px9Wybr9maQI0pJUqhIYuPFeNkJjmy1waiLS9rOrBudcfpysBAqs10rA/exec";

/** 本地限流：一小时最多 5 次真正发出的尝试（含失败），避免误触或脚本刷接口。 */
export const MAX_SENDS_PER_WINDOW = 5;
export const SEND_WINDOW_MS = 60 * 60 * 1000;
export const RATE_LIMITED = "rate_limited";
const STORE_KEY = "clash-assist.feedbackSends";
const REDIRECT_STATUS = new Set([301, 302, 303, 307, 308]);
const MAX_REDIRECTS = 5;

export type PostOnceResult = {
  status: number;
  location: string | null;
  body: string;
};

/** 一次 POST，不跟随重定向。body 必须原样送出。 */
export type PostOnce = (url: string, body: string) => Promise<PostOnceResult>;

export type KeyValueStore = {
  get(key: string): string | null;
  set(key: string, value: string): void;
};

export type SendInput = {
  version: string;
  os: string;
  client: string;
  report: string;
  contact?: string;
};

export type SendResult = {
  ok: boolean;
  report: string;
  error?: string;
  retryAfterMs?: number;
};

export function browserStore(): KeyValueStore {
  return {
    get(key) {
      try {
        return globalThis.localStorage?.getItem(key) ?? null;
      } catch {
        return null;
      }
    },
    set(key, value) {
      try {
        globalThis.localStorage?.setItem(key, value);
      } catch {
        /* 存不下就不跨重启记住；这一次的内存判断仍然有效 */
      }
    },
  };
}

export function readSendStamps(store: KeyValueStore, now: number): number[] {
  let parsed: unknown;
  try {
    parsed = JSON.parse(store.get(STORE_KEY) ?? "[]");
  } catch {
    parsed = [];
  }
  if (!Array.isArray(parsed)) return [];
  return parsed
    .filter((n): n is number => typeof n === "number" && Number.isFinite(n))
    .filter((n) => now - n < SEND_WINDOW_MS)
    .sort((a, b) => a - b);
}

export function rateLimitState(
  stamps: readonly number[],
  now: number,
): { allowed: boolean; retryAfterMs: number } {
  if (stamps.length < MAX_SENDS_PER_WINDOW) return { allowed: true, retryAfterMs: 0 };
  const oldest = stamps[0] ?? now;
  return { allowed: false, retryAfterMs: Math.max(0, oldest + SEND_WINDOW_MS - now) };
}

function writeStamps(store: KeyValueStore, stamps: readonly number[]): void {
  try {
    store.set(STORE_KEY, JSON.stringify(stamps));
  } catch {
    /* 同 browserStore.set */
  }
}

/** 跟随 301/302/303/307/308，方法保持 POST，正文保持调用方传入的 JSON。 */
export async function postJsonFollow(
  startUrl: string,
  body: string,
  postOnce: PostOnce,
): Promise<PostOnceResult> {
  let url = startUrl;
  for (let hop = 0; hop <= MAX_REDIRECTS; hop++) {
    const res = await postOnce(url, body);
    const location = res.location?.trim();
    if (REDIRECT_STATUS.has(res.status) && location) {
      if (hop === MAX_REDIRECTS) return res;
      url = new URL(location, url).href;
      continue;
    }
    return res;
  }
  return { status: 0, location: null, body: "" };
}

/** HTTP 200 且 `{ok:false}` 是失败，不能当成发送成功。 */
export function interpretFeedback(status: number, body: string): { ok: true } | { ok: false; error: string } {
  let parsed: unknown = null;
  try {
    parsed = JSON.parse(body);
  } catch {
    parsed = null;
  }
  if (parsed && typeof parsed === "object" && "ok" in parsed) {
    const record = parsed as { ok?: unknown; error?: unknown };
    if (record.ok === true && status >= 200 && status < 300) return { ok: true };
    if (record.ok === false) {
      const message = typeof record.error === "string" ? record.error.trim() : "";
      return { ok: false, error: message || "rejected" };
    }
  }
  if (status < 200 || status >= 300) return { ok: false, error: `HTTP ${status}` };
  return { ok: false, error: "unexpected response" };
}

function payloadJson(input: SendInput): string {
  const payload: {
    version: string;
    os: string;
    client: string;
    report: string;
    contact?: string;
  } = {
    version: input.version,
    os: input.os,
    client: input.client,
    report: input.report,
  };
  const contact = cleanContact(input.contact ?? "");
  if (contact) payload.contact = contact;
  return JSON.stringify(payload);
}

/**
 * 发送一份已经清洗过的报告。无论成功、失败还是被限流，返回值里的 report
 * 都是入参那一份，供界面继续展示并重试。
 */
export async function sendDiagnostic(
  input: SendInput,
  deps: {
    postOnce: PostOnce;
    store?: KeyValueStore;
    now?: number;
    url?: string;
  },
): Promise<SendResult> {
  const report = input.report;
  if (!input.version || !input.os || !input.client || !report) {
    return { ok: false, error: "incomplete", report };
  }
  const now = deps.now ?? Date.now();
  const store = deps.store ?? browserStore();
  const stamps = readSendStamps(store, now);
  const limit = rateLimitState(stamps, now);
  if (!limit.allowed) {
    return { ok: false, error: RATE_LIMITED, retryAfterMs: limit.retryAfterMs, report };
  }
  const remember = () => {
    writeStamps(store, [...stamps, now].filter((n) => now - n < SEND_WINDOW_MS));
  };
  try {
    const res = await postJsonFollow(deps.url ?? FEEDBACK_URL, payloadJson(input), deps.postOnce);
    remember();
    const interpreted = interpretFeedback(res.status, res.body);
    if (!interpreted.ok) {
      return { ok: false, error: sanitizeText(interpreted.error), report };
    }
    return { ok: true, report };
  } catch (err) {
    remember();
    const message = err instanceof Error ? err.message : String(err ?? "send failed");
    return { ok: false, error: sanitizeText(message), report };
  }
}
