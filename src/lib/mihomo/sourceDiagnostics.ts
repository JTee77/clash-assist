/**
 * Windows 发现来源标签 → 诊断提示（windows-plan §2.7「新增诊断展示」）。
 * 标签由 Rust `mihomo/discovery.rs::discover_controller_inner` 生成：
 * - `verge-pipe:<name>`：走可信控制管道
 * - `verge-pipe-untrusted:<files>`：有管道候选不可信（M-3），停止发现、不回落 TCP
 * - `verge-tcp(<项>[,<项>…]):<files>`，项如 listener-verified / listener-unverified / pipe-insecure /
 *   rejected:<code>（code ∈ listener-conflict / token-mismatch / image-mismatch）
 * - `defaults` / `verge-config:*` / `verge-ec-blank:*`
 *
 * 以上任何一个后面都可能跟若干 `;key=value` 后缀（PR52 Ellie must-fix 1）：
 * - `pipe-insecure-dacl=<a|b>`：有管道因 DACL 允许他人冒充被拒（InsecureDacl），
 *   即使最终选中了别的候选也保留（Ellie PR50 Q5）
 * - `ipv6-dual-stack=<port>`、`pipe-open-to-all=<a|b>`：Pit 后续后端 PR，前端暂不展示
 * - 未知键一律忽略
 * 值里的 `; = | , )` 由 Rust 百分号编码，前端逐项 decodeURIComponent；解码失败视为解析失败。
 */

export const INSECURE_DACL_KEY = "pipe-insecure-dacl";

export type TcpRejectCode = "listener-conflict" | "token-mismatch" | "image-mismatch";

export type SourceNotice =
  | { kind: "insecure-dacl"; pipes: string[] }
  | { kind: "untrusted-pipe" }
  | { kind: "tcp-unverified" }
  | { kind: "tcp-rejected"; code: string };

export type ParsedSource = {
  /** 去掉所有 `;key=value` 后缀后的发现结论本身。 */
  base: string;
  /** `verge-tcp(...)` 括号内按 `,` 拆出的项；不是 TCP 标签时为 null。 */
  tcpItems: string[] | null;
  /** 出现了 `pipe-insecure-dacl` 后缀（无论值能否解析）。 */
  insecureDacl: boolean;
  /** `pipe-insecure-dacl` 的值解码后的管道名；值缺失或解析失败时为空（提示照常显示）。 */
  insecurePipes: string[];
};

const TCP_LABEL = /^verge-tcp\(([^)]*)\)/;
/** 值里不应出现的原始分隔符（Rust 侧会编码它们）。 */
const RAW_RESERVED = /[=,)]/;

/** 百分号解码单个值；出现未编码的保留字符或解码失败返回 null。 */
function decodeValue(raw: string): string | null {
  if (RAW_RESERVED.test(raw)) return null;
  try {
    return decodeURIComponent(raw);
  } catch {
    return null;
  }
}

/** `a|b` 列表：任一项解析失败或为空则整体失败（返回 null）。 */
function decodeList(raw: string): string[] | null {
  if (!raw) return null;
  const out: string[] = [];
  for (const part of raw.split("|")) {
    const v = decodeValue(part);
    if (v === null || !v.trim()) return null;
    out.push(v);
  }
  return out;
}

/** 结构化解析一个来源标签。 */
export function parseSource(source: string | null | undefined): ParsedSource {
  const segments = (source ?? "").split(";");
  const base = segments[0] ?? "";
  const tcp = base.match(TCP_LABEL)?.[1];
  const tcpItems = tcp === undefined ? null : tcp.split(",").map((p) => p.trim()).filter(Boolean);
  let insecureDacl = false;
  const insecurePipes: string[] = [];
  for (const seg of segments.slice(1)) {
    const eq = seg.indexOf("=");
    const key = (eq < 0 ? seg : seg.slice(0, eq)).trim();
    if (key !== INSECURE_DACL_KEY) continue; // 未知键与暂不展示的键忽略
    insecureDacl = true; // 值解析失败也要提示（fail safe）
    if (eq < 0) continue;
    const pipes = decodeList(seg.slice(eq + 1));
    if (!pipes) continue;
    for (const p of pipes) if (!insecurePipes.includes(p)) insecurePipes.push(p);
  }
  return { base, tcpItems, insecureDacl, insecurePipes };
}

/** 来源里是否记录了 InsecureDacl：`pipe-insecure-dacl` 后缀，或 TCP 括号里恰好有一项 `pipe-insecure`。 */
export function hasInsecureDacl(source: string | null | undefined): boolean {
  const p = parseSource(source);
  return p.insecureDacl || (p.tcpItems?.includes("pipe-insecure") ?? false);
}

/** 去掉所有 `;key=value` 后缀，得到发现结论本身。 */
export function baseSource(source: string | null | undefined): string {
  return parseSource(source).base;
}

/** `pipe-insecure-dacl` 后缀里记录的不安全管道名（只取这个键的值）。 */
export function insecurePipesOf(source: string | null | undefined): string[] {
  return parseSource(source).insecurePipes;
}

/**
 * 汇总一次刷新里所有候选的来源标签，返回去重后的提示（InsecureDacl 排最前）。
 * 传入多个来源：自动嗅探时最终选中的候选可能不是带 InsecureDacl 的那个。
 */
export function sourceNoticesOf(sources: readonly (string | null | undefined)[]): SourceNotice[] {
  const pipes: string[] = [];
  let insecure = false;
  let untrusted = false;
  let unverified = false;
  const rejected: string[] = [];
  for (const raw of sources) {
    if (!raw) continue;
    const p = parseSource(raw);
    if (p.insecureDacl || p.tcpItems?.includes("pipe-insecure")) {
      insecure = true;
      for (const name of p.insecurePipes) if (!pipes.includes(name)) pipes.push(name);
    }
    if (p.base.startsWith("verge-pipe-untrusted:")) untrusted = true;
    if (p.tcpItems) {
      if (p.tcpItems.includes("listener-unverified")) unverified = true;
      const rej = p.tcpItems.find((i) => i.startsWith("rejected:"));
      if (rej) {
        const code = rej.slice("rejected:".length);
        if (code && !rejected.includes(code)) rejected.push(code);
      }
    }
  }
  const out: SourceNotice[] = [];
  if (insecure) out.push({ kind: "insecure-dacl", pipes });
  if (untrusted) out.push({ kind: "untrusted-pipe" });
  if (unverified) out.push({ kind: "tcp-unverified" });
  for (const code of rejected) out.push({ kind: "tcp-rejected", code });
  return out;
}
