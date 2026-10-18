//! AI 服务解锁粗检（Gemini / ChatGPT）与 UnlockResult→卡片构造。
import { fetchTextViaProxy } from "../fetchVia";
import { UA } from "./probe";
import type { CheckCard, CheckLevel, UnlockResult } from "../types";

const ISO3_TO_ISO2: Record<string, string> = {
  USA: "US",
  GBR: "GB",
  JPN: "JP",
  SGP: "SG",
  TWN: "TW",
  KOR: "KR",
  DEU: "DE",
  FRA: "FR",
  NLD: "NL",
  AUS: "AU",
  CAN: "CA",
  EST: "EE",
  IND: "IN",
  MYS: "MY",
  THA: "TH",
  PHL: "PH",
  TUR: "TR",
  ARG: "AR",
  BRA: "BR",
  CHE: "CH",
  SWE: "SE",
  CHN: "CN",
  HKG: "HK",
  MAC: "MO",
  RUS: "RU",
  BLR: "BY",
};

const GEMINI_BLOCKED_REGIONS = new Set([
  "CN",
  "CHN",
  "HK",
  "HKG",
  "MO",
  "MAC",
  "RU",
  "RUS",
  "BY",
  "BLR",
  "KP",
  "IR",
  "SY",
  "CU",
]);

export function extractGeminiRegion(out: string): string | null {
  const idx = out.indexOf('"vXmutd"');
  if (idx !== -1) {
    const chunk = out.slice(idx, idx + 80);
    const m = chunk.match(/\\?"([A-Z]{2})\\?"/);
    if (m && m[1] !== "ZZ") return m[1];
  }
  const m3 = out.match(/,2,1,200,"([A-Z]{3})"/);
  if (m3?.[1] && m3[1] !== "ZZZ") {
    return ISO3_TO_ISO2[m3[1]] ?? m3[1];
  }
  return null;
}

export async function probeGeminiUnlock(
  mixedPort?: number | null,
  signal?: AbortSignal,
): Promise<UnlockResult> {
  const r = await fetchTextViaProxy("https://gemini.google.com/app", {
    mixedPort: mixedPort ?? null,
    userAgent: UA,
    timeoutMs: 5000,
    signal,
  });
  const out = r.text;
  const lower = out.toLowerCase();

  const blockedByText = [
    "isn't currently supported in your country",
    "not supported in your country",
    "is not supported in this region",
    "unavailable in your country",
    "gemini isn't available",
    "不支持你所在的地区",
    "不支援你所在的地區",
    "目前不支援你所在的國家",
  ].some((kw) => lower.includes(kw.toLowerCase()));

  const redirectedToFaq =
    /rel=["']canonical["'][^>]+gemini\.google\.com\/faq/i.test(out) ||
    /property=["']og:url["'][^>]+gemini\.google\.com\/faq/i.test(out) ||
    lower.includes("support.google.com/gemini/answer/13575153");

  const detectedCountry = extractGeminiRegion(out);
  const blockedByRegion =
    detectedCountry != null && GEMINI_BLOCKED_REGIONS.has(detectedCountry);

  const probed = [
    "gemini.google.com/app（看是否地区拦截、内容是否正常返回）",
  ];
  const notProbed: string[] = [];

  if (blockedByText || redirectedToFaq || blockedByRegion) {
    return {
      supported: false,
      level: "blocked",
      region: "BLOCKED",
      status: "不可用（地区限制）",
      lines: ["不可用"],
      probed,
      notProbed,
    };
  }

  const hasAppShell =
    out.length > 50000 ||
    ((r.ok || r.status === 200) &&
      out.length > 10000 &&
      (/WIZ_global_data|SNlM0e|bard-web|cfb2h/i.test(out) ||
        /<title>[^<]*Gemini/i.test(out)));

  if (hasAppShell) {
    return {
      supported: true,
      level: "full",
      region: detectedCountry ?? "OK",
      status: detectedCountry ? `可用（${detectedCountry}）` : "可用",
      lines: ["可用"],
      probed,
      notProbed,
    };
  }
  if (!r.ok && !out) {
    return {
      supported: false,
      level: "unknown",
      region: null,
      status: "超时未响应",
      lines: ["超时未响应"],
      probed,
      notProbed,
    };
  }
  return {
    supported: false,
    level: "blocked",
    region: null,
    status: "不可用",
    lines: ["不可用"],
    probed,
    notProbed,
  };
}

export async function probeChatgptUnlock(
  mixedPort?: number | null,
  signal?: AbortSignal,
): Promise<UnlockResult> {
  let loc: string | null = null;
  const trace = await fetchTextViaProxy("https://chatgpt.com/cdn-cgi/trace", {
    mixedPort: mixedPort ?? null,
    userAgent: "Mozilla/5.0",
    timeoutMs: 3000,
    signal,
  });
  for (const line of trace.text.split("\n")) {
    if (line.startsWith("loc=")) {
      loc = line.split("=")[1]?.trim() ?? null;
      break;
    }
  }

  let webOk = false;
  const web = await fetchTextViaProxy(
    "https://api.openai.com/compliance/cookie_requirements",
    {
      mixedPort: mixedPort ?? null,
      userAgent: UA,
      timeoutMs: 3500,
      signal,
    },
  );
  if (web.text && !web.text.includes("unsupported_country")) {
    if (web.status === 200 || (web.status !== 403 && web.status !== 0)) {
      webOk = true;
    }
  }
  // CORS may block body in browser — treat opaque network success carefully
  if (!web.ok && web.status === 0) {
    // likely CORS; try chatgpt.com homepage as soft signal
    const home = await fetchTextViaProxy("https://chatgpt.com/", {
      mixedPort: mixedPort ?? null,
      userAgent: UA,
      timeoutMs: 4000,
      signal,
    });
    if (home.ok || home.status === 200 || home.text.length > 1000) {
      webOk = !home.text.includes("unsupported_country");
    }
  }

  const locTag = loc ?? "未知地区";
  const probed = [
    "chatgpt.com/cdn-cgi/trace、OpenAI compliance 接口（必要时再看 chatgpt.com 首页）",
  ];
  const notProbed: string[] = [];

  if (!webOk) {
    return {
      supported: false,
      level: "blocked",
      region: loc,
      status: loc ? `不可用（${locTag}）` : "不可用",
      lines: ["不可用"],
      probed,
      notProbed,
    };
  }
  return {
    supported: true,
    level: "full",
    region: loc,
    status: loc ? `可用（${locTag}）` : "可用",
    lines: ["可用"],
    probed,
    notProbed,
  };
}

function unlockCard(
  id: string,
  title: string,
  result: UnlockResult,
): CheckCard {
  let level: CheckLevel = "fail";
  if (result.supported) {
    level = "pass";
    if (result.level === "web_only" || result.level === "app_only") level = "warn";
    if (result.level === "full") level = "pass";
  } else if (result.level === "unknown") {
    level = "unknown";
  }

  const lines = (result.lines ?? []).join("；");
  const probed = (result.probed ?? []).join("；");
  const notProbed = (result.notProbed ?? []).join("；");

  const processParts = [
    lines ? `结果对照：${lines}` : "",
    probed ? `测了什么：${probed}` : "",
    notProbed ? `没测什么：${notProbed}` : "",
    result.region ? `出口提示地区：${result.region}` : "",
  ].filter(Boolean);

  const suggestion: string | undefined = undefined;

  return {
    id,
    title,
    level,
    conclusion: result.status,
    process: processParts.join("\n"),
    suggestion,
    metrics: {
      unlockSupported: result.supported,
      unlockLevel: result.level ?? null,
      unlockRegion: result.region,
    },
  };
}

export { unlockCard };
