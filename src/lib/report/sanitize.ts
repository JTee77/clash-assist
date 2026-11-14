/**
 * 诊断导出和「发送错误报告」共用的唯一清洗实现。
 * 版本、系统、架构、客户端、错误码、检查项名称、通过/失败、节点数量、
 * 是否切回原节点，由报告组装器写成结构化字段，不从自由文本里回捞。
 */

export type NameRedaction = { name: string; replacement: string };

const URL_RE = /[a-z][a-z0-9+.-]*:\/\/[^\s<>"'「」『』]+/gi;
/**
 * 家目录前缀。分隔符可以是 `/`、`\`，或 `%2F` / `%5C`。
 * 用 sticky 正则从指定下标匹配，避免每次切片。
 */
const SEP = String.raw`(?:[/\\]|%2[Ff]|%5[Cc])`;
const DRIVE_USERS = new RegExp(`[A-Za-z](?::|%3[Aa])${SEP}Users${SEP}`, "iy");
const UNIX_USERS = new RegExp(`/(?:Users|home)${SEP}`, "iy");
const UNIX_ROOT = /\/var\/root(?![A-Za-z0-9_%])/iy;
const ENC_USERS = /%2[Ff](?:Users|home)%2[Ff]/iy;
const ENC_ROOT = /%2[Ff]var%2[Ff]root%2[Ff]/iy;
const TILDE = new RegExp(`~${SEP}`, "iy");
const WIN_ENV = /%(?:USERPROFILE|HOMEPATH|HOMEDRIVE)%/iy;
const UNIX_ENV = /\$(?:\{HOME\}|HOME(?![A-Za-z0-9_]))/y;
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
  /\b(?:bearer|access[_-]?token|token|secret|api[_-]?key|authorization|password|passwd|username|user|uuid)\b\s*[:=]\s*\S+/gi;
/** `Cookie:` 的值可以是 `a=b; c=d`。分号后面的空格仍算在值里，普通句子不算。 */
const COOKIE_RE = /\b(?:set-)?cookie\b\s*[:=]\s*[^;\s]+(?:\s*;\s*[^;\s]+)*/gi;
const SECRET_QUERY_RE =
  /([?&#])(?:access[_-]?token|token|secret|key|password|passwd|auth|authorization)=(?:[^&#\s]+)/gi;
const HEX_SECRET_RE = /\b[a-fA-F0-9]{32,}\b/g;
/** `vmess:eyJ…` 这种没有 `://` 的分享片段。`vmess://` 已由 URL 规则吃掉。 */
const SCHEMELESS_PROXY_RE =
  /\b(?:vmess|vless|trojan|ssr|hysteria2|hy2):(?!\/\/)[A-Za-z0-9+/_=-]{8,}/gi;
/** 至少 24 个字符的标准 base64。结尾的 `=` 不能用 `\b`，它和前后都不是单词字符。 */
const B64_RE =
  /\b(?:[A-Za-z0-9+/]{4}){6,}(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?(?![A-Za-z0-9+/])/g;
const HOST_RE =
  /\b(?:[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?\.)+(?:[a-zA-Z]{2,24})\b/g;
const CJK_RE = /[\u3400-\u9fff\u3040-\u30ff\uac00-\ud7af\u3000-\u303f\uff00-\uffef]/;

const HARD_STOP = new Set([
  "\n",
  "\r",
  "\t",
  '"',
  "'",
  "`",
  "「",
  "」",
  "『",
  "』",
  "“",
  "”",
  "<",
  ">",
  "|",
  ",",
  ";",
  "，",
  "。",
  "；",
  "：",
  "、",
  "！",
  "？",
  "（",
  "）",
]);

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

function matchLen(re: RegExp, s: string, i: number): number {
  re.lastIndex = i;
  const m = re.exec(s);
  if (!m || m.index !== i) return 0;
  return m[0].length;
}

function sepLen(s: string, i: number): number {
  const c = s[i];
  if (c === "/" || c === "\\") return 1;
  if (i + 3 <= s.length) {
    const e = s.slice(i, i + 3).toLowerCase();
    if (e === "%2f" || e === "%5c") return 3;
  }
  return 0;
}

function isCjk(c: string): boolean {
  return CJK_RE.test(c);
}

function stopsPath(c: string): boolean {
  return HARD_STOP.has(c) || c === "?" || c === "*" || c === "#";
}

/**
 * 从家目录 / UNC 前缀之后继续吃路径。
 * 空格不截断：后面还有分隔符，或当前段是用户名（`Users` / `home` 的下一段）时，空格算在路径里。
 * 用户名段不吞后面的中文句子。
 */
function consumePath(s: string, start: number, usernameSegment: boolean): number {
  let i = start;
  let segmentStart = start;
  let username = usernameSegment;
  while (i < s.length) {
    const sep = sepLen(s, i);
    if (sep > 0) {
      const seg = s.slice(segmentStart, i);
      i += sep;
      segmentStart = i;
      username = /^(?:users|home)$/i.test(seg);
      continue;
    }
    const c = s[i]!;
    if (stopsPath(c)) break;
    if (c === " ") {
      let j = i + 1;
      while (j < s.length && s[j] === " ") j++;
      if (j >= s.length || stopsPath(s[j]!)) break;
      let foundSep = false;
      for (let k = j; k < s.length; k++) {
        const ck = s[k]!;
        if (stopsPath(ck)) break;
        if (sepLen(s, k) > 0) {
          foundSep = true;
          break;
        }
      }
      if (foundSep) {
        i = j;
        continue;
      }
      // 没有后续分隔符时，只再吃一个词（「Alice Smith」的姓），不把后面的英文句子吞掉。
      if (username && !isCjk(s[j]!)) {
        i = j;
        while (i < s.length && s[i] !== " " && !stopsPath(s[i]!)) i += 1;
        username = false;
        continue;
      }
      break;
    }
    i += 1;
  }
  return i;
}

function atTokenBoundary(s: string, i: number): boolean {
  if (i === 0) return true;
  const prev = s[i - 1]!;
  if (isCjk(prev) || stopsPath(prev) || prev === " " || prev === "/" || prev === "\\") return true;
  return /[()[\]{}=:]/.test(prev);
}

type PathHit = { len: number; usernameNext: boolean; unc: boolean };

function matchUnc(s: string, i: number): PathHit | null {
  const back = s[i] === "\\" && s[i + 1] === "\\";
  // `https://` 里的 `//` 前面是冒号，留给 URL 规则。其余 `//server/share` 当 UNC。
  const fwd = s[i] === "/" && s[i + 1] === "/" && (i === 0 || s[i - 1] !== ":");
  if (!back && !fwd) return null;
  if (back) {
    const third = s[i + 2];
    if (third === "?" || third === ".") {
      if (s[i + 3] !== "\\") return null;
      return { len: 4, usernameNext: false, unc: true };
    }
  }
  const third = s[i + 2];
  if (!third || third === "\\" || third === "/" || stopsPath(third)) return null;
  return { len: 2, usernameNext: false, unc: true };
}

function matchHome(s: string, i: number): PathHit | null {
  const c = s[i];
  if (c === "/" || c === "%") {
    let n = matchLen(UNIX_USERS, s, i);
    if (n) return { len: n, usernameNext: true, unc: false };
    n = matchLen(UNIX_ROOT, s, i);
    if (n) return { len: n, usernameNext: false, unc: false };
    n = matchLen(ENC_USERS, s, i);
    if (n) return { len: n, usernameNext: true, unc: false };
    n = matchLen(ENC_ROOT, s, i);
    if (n) return { len: n, usernameNext: false, unc: false };
  }
  if (c === "~") {
    const n = matchLen(TILDE, s, i);
    if (n) return { len: n, usernameNext: false, unc: false };
  }
  if (c === "%") {
    const n = matchLen(WIN_ENV, s, i);
    if (n) return { len: n, usernameNext: false, unc: false };
  }
  if (c === "$") {
    const n = matchLen(UNIX_ENV, s, i);
    if (n) return { len: n, usernameNext: false, unc: false };
  }
  if (c && /[A-Za-z]/.test(c) && atTokenBoundary(s, i)) {
    const n = matchLen(DRIVE_USERS, s, i);
    if (n) return { len: n, usernameNext: true, unc: false };
  }
  return null;
}

function spanHasSep(s: string, from: number, to: number): boolean {
  for (let i = from; i < to; i += 1) {
    if (sepLen(s, i) > 0) return true;
  }
  return false;
}

/**
 * 家目录、UNC、`%USERPROFILE%` / `$HOME`、以及百分号编码的同一类路径。
 * 不删句子里的无点主机名（`fileserver`、`Clash`）：误伤太大。UNC 的服务器名随整段路径一起去掉。
 */
function redactPaths(input: string): string {
  let out = "";
  let i = 0;
  while (i < input.length) {
    const hit = matchUnc(input, i) ?? matchHome(input, i);
    if (!hit) {
      out += input[i];
      i += 1;
      continue;
    }
    const end = consumePath(input, i + hit.len, hit.usernameNext);
    if (hit.unc && !spanHasSep(input, i + hit.len, end)) {
      out += input[i];
      i += 1;
      continue;
    }
    out += "[path]";
    i = Math.max(end, i + hit.len);
  }
  return out;
}

function isSchemelessProxyBlob(token: string): boolean {
  // `{"…` 的 base64。vmess 分享体和 JWT 片段都是这种开头。
  if (token.startsWith("eyJ")) return true;
  let decoded: string;
  try {
    decoded = atob(token);
  } catch {
    return false;
  }
  if (/"\s*(?:add|ps|id|port|tls|net|v|host)\s*"\s*:/.test(decoded)) return true;
  return /^[A-Za-z0-9.+-]+:[^@\s]+@[^:\s]+(?::\d+)?/.test(decoded);
}

function redactProxyBlobs(input: string): string {
  return input.replace(B64_RE, (token) => (isSchemelessProxyBlob(token) ? "[secret]" : token));
}

/** 只去掉 URL 和密钥。联系方式要保留邮箱，所以不走完整清洗。 */
export function stripUrlsAndSecrets(input: string): string {
  return redactProxyBlobs(
    input
      .replace(URL_RE, "[url]")
      .replace(SCHEMELESS_PROXY_RE, "[secret]")
      .replace(COOKIE_RE, "[secret]")
      .replace(SECRET_LABEL_RE, "[secret]")
      .replace(SECRET_QUERY_RE, "$1[secret]")
      .replace(HEX_SECRET_RE, "[secret]"),
  );
}

/** 自由文本清洗。占位符是固定英文标记，方便测试锁定「去掉了什么」。 */
export function sanitizeText(input: string, names: readonly NameRedaction[] = []): string {
  let s = redactNames(input, names);
  s = redactPaths(s);
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
