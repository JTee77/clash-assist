/**
 * Windows 发现来源标签 → 诊断提示（windows-plan §2.7「新增诊断展示」）。
 * 标签由 Rust `mihomo/discovery.rs::discover_controller_inner` 生成：
 * - `verge-pipe:<name>`：走可信控制管道
 * - `verge-pipe-untrusted:<files>`：有管道候选不可信（M-3），停止发现、不回落 TCP
 * - `verge-tcp(listener-verified|listener-unverified[,pipe-insecure]):<files>`
 * - `verge-tcp(rejected:<code>):<files>`，code ∈ listener-conflict / token-mismatch / image-mismatch
 * - `defaults` / `verge-config:*` / `verge-ec-blank:*`
 * - 以上任何一个都可能带后缀 `;pipe-insecure-dacl=<a|b>`：有管道因 DACL 允许他人冒充被拒（InsecureDacl），
 *   即使最终选中了别的候选也保留（Ellie PR50 Q5）。
 */

export const INSECURE_DACL_SUFFIX = ";pipe-insecure-dacl=";

export type TcpRejectCode = "listener-conflict" | "token-mismatch" | "image-mismatch";

export type SourceNotice =
  | { kind: "insecure-dacl"; pipes: string[] }
  | { kind: "untrusted-pipe" }
  | { kind: "tcp-unverified" }
  | { kind: "tcp-rejected"; code: string };

/** 来源里是否记录了 InsecureDacl（`;pipe-insecure-dacl=` 后缀或 TCP 标签里的 `pipe-insecure`）。 */
export function hasInsecureDacl(source: string | null | undefined): boolean {
  const s = source ?? "";
  return s.includes(INSECURE_DACL_SUFFIX) || s.includes("pipe-insecure");
}

/** 去掉 `;pipe-insecure-dacl=…` 后缀，得到发现结论本身。 */
export function baseSource(source: string | null | undefined): string {
  const s = source ?? "";
  const i = s.indexOf(INSECURE_DACL_SUFFIX);
  return i < 0 ? s : s.slice(0, i);
}

/** 后缀里记录的不安全管道名（`a|b`）。 */
export function insecurePipesOf(source: string | null | undefined): string[] {
  const s = source ?? "";
  const i = s.indexOf(INSECURE_DACL_SUFFIX);
  if (i < 0) return [];
  return s
    .slice(i + INSECURE_DACL_SUFFIX.length)
    .split(/[|;]/)
    .map((p) => p.trim())
    .filter(Boolean);
}

const TCP_LABEL = /^verge-tcp\(([^)]*)\)/;

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
    const s = raw ?? "";
    if (!s) continue;
    if (hasInsecureDacl(s)) {
      insecure = true;
      for (const p of insecurePipesOf(s)) if (!pipes.includes(p)) pipes.push(p);
    }
    const base = baseSource(s);
    if (base.startsWith("verge-pipe-untrusted:")) untrusted = true;
    const tcp = base.match(TCP_LABEL)?.[1];
    if (tcp) {
      const parts = tcp.split(",").map((p) => p.trim());
      if (parts.includes("listener-unverified")) unverified = true;
      const rej = parts.find((p) => p.startsWith("rejected:"));
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
