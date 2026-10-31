export type CheckLevel = "pass" | "warn" | "fail" | "unknown" | "running";

/** Structured numbers/flags for scoring & UI — prefer over parsing conclusion text. */
export type CheckCardMetrics = {
  /** Bandwidth sample (Mbps). */
  downMbps?: number | null;
  upMbps?: number | null;
  /** Latency sample (ms). */
  latencyMs?: number | null;
  /** Exit IP card. */
  exitIp?: string | null;
  countryCode?: string | null;
  hosting?: boolean | null;
  /** Unlock / service cards. */
  unlockSupported?: boolean;
  unlockLevel?: string | null;
  unlockRegion?: string | null;
};

export interface CheckCard {
  id: string;
  title: string;
  level: CheckLevel;
  /** Always-visible plain-Chinese one-sentence result */
  conclusion: string;
  /** Technical detail (URLs, HTTP codes, timings…); collapsed by default in UI */
  process?: string;
  /** Always-visible next-step tip (actionable; omit when nothing useful) */
  suggestion?: string;
  /** @deprecated Prefer `conclusion`. Kept for migration / older callers. */
  summary?: string;
  /** @deprecated Prefer `process`. */
  detail?: string;
  /** @deprecated Prefer `suggestion`. */
  tip?: string;
  /**
   * True when the check could not actually run through the proxy (no mixed-port
   * available) and we REFUSED to fall back to a direct browser fetch, so the
   * result is neither good nor bad — genuinely "未验证". Scoring drops such a
   * dimension from the weighted average rather than treating it as a failure.
   */
  unverified?: boolean;
  /** Machine-readable probe outputs; scoring reads these instead of parsing copy. */
  metrics?: CheckCardMetrics;
  /**
   * Windows DNS 卡的网卡摘要（后端 raw_hint，含网卡名、描述、内网 DNS）。只用于展示：
   * 不拼进 `process`，所以评分的关键词匹配读不到它（PR #51 M-4：中文 Windows 的
   * PPPoE 网卡叫「宽带连接」）；写结果缓存前会被去掉（PR #51 S-3）。
   * macOS 的 scutil 原文仍按原样放在 `process` 里，行为不变。
   */
  adapterHint?: string;
}

/** 卡片「过程」区展示的全文：`process`（或旧字段 `detail`）后接网卡摘要。 */
export function cardProcessText(card: CheckCard): string | undefined {
  const base = card.process ?? card.detail;
  if (!card.adapterHint) return base;
  const hint = `网卡摘要：\n${card.adapterHint}`;
  return base ? `${base}\n${hint}` : hint;
}

export interface ExitIpInfo {
  ip: string | null;
  country: string | null;
  countryCode: string | null;
  org: string | null;
  isp: string | null;
  hosting: boolean | null;
  /** true when `hosting` was keyword-inferred (TLS sources don't provide it) */
  hostingInferred?: boolean;
  ipTypeLabel: string;
}

export interface UnlockResult {
  supported: boolean;
  /** Internal machine level: full | web_only | app_only | blocked | unknown */
  level?: string;
  region: string | null;
  /** Short headline for the card conclusion (plain Chinese, no jargon) */
  status: string;
  /** One-line breakdowns shown in process */
  lines?: string[];
  /** What we probed */
  probed?: string[];
  /** What we explicitly did not probe */
  notProbed?: string[];
}

export interface EgressReport {
  ranAt: string;
  cards: CheckCard[];
  exitIp: ExitIpInfo | null;
  gemini: UnlockResult | null;
  chatgpt: UnlockResult | null;
  latencyMs: number | null;
  note: string;
}
