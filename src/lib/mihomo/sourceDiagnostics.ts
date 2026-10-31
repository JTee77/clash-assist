/**
 * Windows 发现来源标签 → 诊断提示（windows-plan §2.7「新增诊断展示」）。
 * 标签由 Rust `mihomo/discovery.rs::discover_controller_inner` 生成：
 * - `verge-pipe:<name>`：走可信控制管道
 * - `verge-pipe-untrusted:<files>`：有管道候选不可信（M-3），停止发现、不回落 TCP
 * - `verge-tcp(<项>[,<项>…]):<files>`，项如 listener-verified / listener-unverified / service-mode /
 *   pipe-insecure / rejected:<code>（code ∈ listener-conflict / token-mismatch / image-mismatch）
 * - `defaults` / `verge-config:*` / `verge-ec-blank:*`
 *
 * 以上任何一个后面都可能跟若干 `;key=value` 后缀（PR52 Ellie must-fix 1）：
 * - `pipe-insecure-dacl=<a|b>`：有管道因 DACL 允许他人冒充被拒（InsecureDacl），
 *   即使最终选中了别的候选也保留（Ellie PR50 Q5）
 * - `ipv6-dual-stack=<port>`（S-R12）：IPv4 回环没有监听者，但同端口有 `[::]` 监听；值为十进制端口
 * - `pipe-open-to-all=<a|b>`（S-2）：选中的管道 DACL 给所有本机用户写权限（只提示，不拒绝）
 * - 未知键一律忽略
 * 后缀顺序（Rust `SuffixKey`）：ipv6-dual-stack、pipe-open-to-all、pipe-insecure-dacl。
 * 值里的 `; = | , ( ) %` 与控制字符由 Rust 按大写十六进制百分号编码，前端逐项 decodeURIComponent；
 * 解码失败视为解析失败：键仍然算出现（提示照常显示），只是不带具体值。
 *
 * 截断（PR #54）：标签超过 1024 字节时，Rust 按整项截断 body 的文件列表和各后缀值，截断处是占位项
 * `%28truncated%29`（解码后 [`TRUNCATED_ITEM`]）。它不是管道名 / 文件名：解析时从管道名列表里剔除。
 * 界面目前不展示管道名 / 文件名，所以不再单独记录「是否截断」（PR #56 Ellie S-1）。
 */

export const INSECURE_DACL_KEY = "pipe-insecure-dacl";
export const IPV6_DUAL_STACK_KEY = "ipv6-dual-stack";
export const PIPE_OPEN_TO_ALL_KEY = "pipe-open-to-all";
/** `verge-tcp(...)` 括号内的服务模式项（C-4）。 */
export const SERVICE_MODE_ITEM = "service-mode";
/** Rust `source_label::TRUNCATED_ITEM`：截断占位项（编码后 `%28truncated%29`），不是名字。 */
export const TRUNCATED_ITEM = "(truncated)";

export type TcpRejectCode = "listener-conflict" | "token-mismatch" | "image-mismatch";

export type SourceNotice =
  | { kind: "insecure-dacl"; pipes: string[] }
  | { kind: "untrusted-pipe" }
  | { kind: "pipe-open-to-all"; pipes: string[] }
  | { kind: "tcp-unverified" }
  | { kind: "service-mode" }
  | { kind: "tcp-rejected"; code: string }
  | { kind: "ipv6-dual-stack"; ports: number[] };

/** 后端 PR #53 新增的三个标记（与 ParsedSource 分开，保持其形状不变）。 */
export type SourceMarkers = {
  /** TCP 括号里有整项 `service-mode`。 */
  serviceMode: boolean;
  /** 出现了 `ipv6-dual-stack` 后缀（无论值能否解析）。 */
  ipv6DualStack: boolean;
  /** `ipv6-dual-stack` 的端口；值缺失或解析失败时为空（提示照常显示，只是不带端口）。 */
  ipv6Ports: number[];
  /** 出现了 `pipe-open-to-all` 后缀（无论值能否解析）。 */
  openToAll: boolean;
  /** `pipe-open-to-all` 的值解码后的管道名（不含截断占位项）；值缺失或解析失败时为空。 */
  openToAllPipes: string[];
};

export type ParsedSource = {
  /** 去掉所有 `;key=value` 后缀后的发现结论本身。 */
  base: string;
  /** `verge-tcp(...)` 括号内按 `,` 拆出的项；不是 TCP 标签时为 null。 */
  tcpItems: string[] | null;
  /** 出现了 `pipe-insecure-dacl` 后缀（无论值能否解析）。 */
  insecureDacl: boolean;
  /** `pipe-insecure-dacl` 的值解码后的管道名（不含截断占位项）；值缺失或解析失败时为空（提示照常显示）。 */
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
    for (const p of pipes) if (p !== TRUNCATED_ITEM && !insecurePipes.includes(p)) insecurePipes.push(p);
  }
  return { base, tcpItems, insecureDacl, insecurePipes };
}

/** 十进制端口 1–65535；其他一律视为解析失败。 */
function parsePort(v: string): number | null {
  if (!/^[0-9]{1,5}$/.test(v)) return null;
  const n = Number(v);
  return n >= 1 && n <= 65535 ? n : null;
}

/** 解析 PR #53 的三个新标记：`service-mode` 括号项、`;ipv6-dual-stack=`、`;pipe-open-to-all=`。 */
export function sourceMarkersOf(source: string | null | undefined): SourceMarkers {
  const segments = (source ?? "").split(";");
  const tcpItems = parseSource(source).tcpItems;
  const out: SourceMarkers = {
    serviceMode: tcpItems?.includes(SERVICE_MODE_ITEM) ?? false,
    ipv6DualStack: false,
    ipv6Ports: [],
    openToAll: false,
    openToAllPipes: [],
  };
  for (const seg of segments.slice(1)) {
    const eq = seg.indexOf("=");
    const key = (eq < 0 ? seg : seg.slice(0, eq)).trim();
    // 截断占位项不是名字 / 端口：剔除
    const values = eq < 0 ? null : (decodeList(seg.slice(eq + 1))?.filter((v) => v !== TRUNCATED_ITEM) ?? null);
    if (key === IPV6_DUAL_STACK_KEY) {
      out.ipv6DualStack = true;
      const ports = values?.map(parsePort);
      if (!ports || ports.some((p) => p === null)) continue; // 解析失败：不带端口
      for (const p of ports as number[]) if (!out.ipv6Ports.includes(p)) out.ipv6Ports.push(p);
    } else if (key === PIPE_OPEN_TO_ALL_KEY) {
      out.openToAll = true;
      if (!values) continue;
      for (const v of values) if (!out.openToAllPipes.includes(v)) out.openToAllPipes.push(v);
    }
  }
  return out;
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
 * 汇总一次刷新里所有候选的来源标签，返回去重后的提示；service-mode 出现时不再单独出 tcp-unverified。顺序：InsecureDacl、不可信管道、
 * 管道对所有用户开放、TCP 未验证、服务模式、TCP 拒绝原因、IPv6 双栈。
 * 传入多个来源：自动嗅探时最终选中的候选可能不是带标记的那个。
 */
export function sourceNoticesOf(sources: readonly (string | null | undefined)[]): SourceNotice[] {
  const pipes: string[] = [];
  let insecure = false;
  let untrusted = false;
  let unverified = false;
  const rejected: string[] = [];
  let openToAll = false;
  const openPipes: string[] = [];
  let serviceMode = false;
  let ipv6 = false;
  const ports: number[] = [];
  for (const raw of sources) {
    if (!raw) continue;
    const p = parseSource(raw);
    const m = sourceMarkersOf(raw);
    if (m.openToAll) {
      openToAll = true;
      for (const name of m.openToAllPipes) if (!openPipes.includes(name)) openPipes.push(name);
    }
    if (m.serviceMode) serviceMode = true;
    if (m.ipv6DualStack) {
      ipv6 = true;
      for (const port of m.ipv6Ports) if (!ports.includes(port)) ports.push(port);
    }
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
  if (openToAll) out.push({ kind: "pipe-open-to-all", pipes: openPipes });
  // 服务模式恒为 listener-unverified（C-4）：两者同时出现时只显示 service-mode，它的文案已包含
  // 「无法验证监听方、已拒绝连接」（Pit，PR #56）。
  if (unverified && !serviceMode) out.push({ kind: "tcp-unverified" });
  if (serviceMode) out.push({ kind: "service-mode" });
  for (const code of rejected) out.push({ kind: "tcp-rejected", code });
  if (ipv6) out.push({ kind: "ipv6-dual-stack", ports });
  return out;
}
