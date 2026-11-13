/**
 * 诊断导出和「发送错误报告」共用的唯一清洗实现。
 * 版本、系统、架构、客户端、错误码、检查项名称、通过/失败、节点数量、
 * 是否切回原节点，由报告组装器写成结构化字段，不从自由文本里回捞。
 */

export type NameRedaction = { name: string; replacement: string };

const URL_RE = /[a-z][a-z0-9+.-]*:\/\/[^\s<>"'「」『』]+/gi;
const HOME_PATH_RES = [
  /\\\\[?]\\[A-Za-z]:\\Users\\[^\s"'「」『』]+/g,
  /[A-Za-z]:[/\\]Users[/\\][^\s"'「」『』]+/g,
  /(?:\/Users\/|\/home\/|\/var\/root\/)[^\s"'「」『』]+/g,
  /~\/[^\s"'「」『』]+/g,
];
const EMAIL_RE = /\b[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}\b/g;
const IPV4_RE =
  /\b(?:(?:25[0-5]|2[0-4]\d|[01]?\d?\d)\.){3}(?:25[0-5]|2[0-4]\d|[01]?\d?\d)\b/g;
/**
 * 至少两段冒号，才能盖住 `2001:db8::1`、`::1`、`fe80::1` 和 `::ffff:192.0.2.1`。
 * 三段版本号（0.3.2）没有冒号，不会被当成地址。
 */
const IPV6_RE =
  /(?<![A-Za-z0-9])(?:[A-Fa-f0-9]{0,4}:){2,7}[A-Fa-f0-9]{0,4}(?:\.(?:25[0-5]|2[0-4]\d|[01]?\d?\d)){0,3}(?![A-Za-z0-9:.])/g;
const SECRET_LABEL_RE =
  /\b(?:bearer|token|secret|api[_-]?key|authorization|password|passwd|username|user)\b\s*[:=]\s*\S+/gi;
const SECRET_QUERY_RE =
  /([?&#])(?:token|secret|key|password|passwd|auth|authorization)=(?:[^&#\s]+)/gi;
const HEX_SECRET_RE = /\b[a-fA-F0-9]{32,}\b/g;
const HOST_RE =
  /\b(?:[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?\.)+(?:[a-zA-Z]{2,24})\b/g;

function escapeRegExp(s: string): string {
  return s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

/** 单字母节点名只在引号旁替换，避免把普通字母吃掉。 */
function allowNameMatch(text: string, index: number, name: string): boolean {
  if (name.length >= 3) return true;
  const before = index === 0 ? "" : text[index - 1]!;
  const after = text[index + name.length] ?? "";
  const boundary = (c: string) => c === "" || /[^\p{L}\p{N}]/u.test(c);
  if (!boundary(before) || !boundary(after)) return false;
  if (name.length === 1) return /["'「」『』“”]/.test(before) || /["'「」『』“”]/.test(after);
  return true;
}

/** 把已知节点名换成调用方给的索引标签（如「节点 1」）。长名优先，避免短名截断长名。 */
export function redactNames(text: string, pairs: readonly NameRedaction[]): string {
  const usable = pairs.filter((p) => p.name.trim().length > 0);
  if (usable.length === 0 || text.length === 0) return text;
  const sorted = [...usable].sort((a, b) => b.name.length - a.name.length);
  const map = new Map(sorted.map((p) => [p.name, p.replacement]));
  const re = new RegExp(sorted.map((p) => escapeRegExp(p.name)).join("|"), "g");
  return text.replace(re, (match, offset: number) => {
    if (!allowNameMatch(text, offset, match)) return match;
    return map.get(match) ?? match;
  });
}

/** 只去掉 URL 和密钥。联系方式要保留邮箱，所以不走完整清洗。 */
export function stripUrlsAndSecrets(input: string): string {
  return input
    .replace(URL_RE, "[url]")
    .replace(SECRET_LABEL_RE, "[secret]")
    .replace(SECRET_QUERY_RE, "$1[secret]")
    .replace(HEX_SECRET_RE, "[secret]");
}

/** 自由文本清洗。占位符是固定英文标记，方便测试锁定「去掉了什么」。 */
export function sanitizeText(input: string, names: readonly NameRedaction[] = []): string {
  let s = redactNames(input, names);
  for (const re of HOME_PATH_RES) s = s.replace(re, "[path]");
  s = stripUrlsAndSecrets(s);
  s = s.replace(EMAIL_RE, "[user]");
  s = s.replace(IPV6_RE, "[ip]");
  s = s.replace(IPV4_RE, "[ip]");
  s = s.replace(HOST_RE, "[host]");
  s = s.replace(/[\u0000-\u0008\u000B\u000C\u000E-\u001F\u007F]/g, "");
  return s;
}

const SEMVER = /^\d+\.\d+\.\d+$/;
const TOKEN = /^[a-z0-9_-]{1,64}$/;

export function keepSemver(value: string): string {
  return SEMVER.test(value) ? value : "unknown";
}

export function keepToken(value: string, fallback = "unknown"): string {
  return TOKEN.test(value) ? value : fallback;
}

/** 用户自愿填写的联系方式：去掉控制字符、订阅 URL 和密钥，保留邮箱。 */
export function cleanContact(raw: string): string | undefined {
  const stripped = stripUrlsAndSecrets(raw.replace(/[\u0000-\u001F\u007F]/g, " "))
    .replace(/\s+/g, " ")
    .trim()
    .slice(0, 200);
  if (!stripped || stripped === "[url]" || stripped === "[secret]") return undefined;
  return stripped;
}
