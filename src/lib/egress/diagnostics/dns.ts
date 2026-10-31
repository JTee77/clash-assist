//! DNS 泄漏检测：UDP whoami 实测出口 IP 与代理出口比对（含旧名别名）。
import { listDnsResolvers, dnsWhoami } from "../fetchVia";
import { probeText } from "./probe";
import { fetchIpCountry } from "./exitIp";
import { localeCountry } from "../leakMatrix";
import type { CheckCard, CheckLevel, ExitIpInfo } from "../types";

/**
 * A1: DNS 泄漏（确定性改造）。
 * 旧版只列 scutil 配置的 resolver，几乎永远判 pass。新版用 UDP 原始 DNS `TXT whoami` 实测
 * DNS 查询真正从哪个公网 IP 出去（以及被哪个递归解析器服务），再与代理出口 IP 比对：
 * 同出口 → 未泄漏；不同 → DNS 绕过了代理、疑似暴露真实地址。scutil 列表与 Cloudflare
 * 启发式降级为过程细节。
 */
/**
 * DNS 出口判定的纯函数核心（可单测）。
 * v0.1.11 修订：v6 普及后"解析出口 ≠ 节点出口 IP"不再等于泄漏——供应商常见
 * v4 节点 + 异地 v6 解析基础设施的组合（实测 xTom HK 节点 + DE 解析），
 * 字符串比对必然误报。改为归属地判定：真正要防的是"解析链路暴露真实 ISP"。
 */
export type DnsEgressJudgeInput = {
  qIp: string;
  exitIp: string;
  /** 解析出口的归属国（geo 服务取得；null = 查不到） */
  qCountry?: string | null;
  /** 真实归属国（系统区域推断；null = 推断不出） */
  realCountry?: string | null;
};

export function judgeDnsEgress(i: DnsEgressJudgeInput): {
  level: CheckLevel;
  conclusion: string;
  suggestion?: string;
} {
  if (i.qIp === i.exitIp) {
    return {
      level: "pass",
      conclusion: `DNS 查询与代理流量同出口（${i.qIp}）—— 未泄漏真实地址`,
    };
  }
  if (i.qCountry && i.realCountry) {
    if (i.qCountry !== i.realCountry) {
      return {
        level: "pass",
        conclusion: `DNS 解析链路从 ${i.qIp}（${i.qCountry}）出去，与代理出口（${i.exitIp}）不同节点；但归属地与真实归属（${i.realCountry}）不同 —— 属供应商基础设施，未泄漏真实地址`,
      };
    }
    return {
      level: "fail",
      conclusion: `DNS 从 ${i.qIp}（${i.qCountry}）出去，与你的真实网络归属（${i.realCountry}）一致，而代理出口是 ${i.exitIp} —— 疑似 DNS 泄漏真实地址`,
      suggestion:
        "把系统/Wi-Fi DNS 改到隧道可达的 1.1.1.1 / 8.8.8.8，或在客户端开启 DNS 劫持（fake-ip / TUN DNS）后重测。",
    };
  }
  return {
    level: "warn",
    conclusion: `DNS 从 ${i.qIp} 出去，而代理出口是 ${i.exitIp}（归属地要素不全，无法完成最终判定）`,
    suggestion: "稍后重试；或到 BrowserLeaks / dnsleaktest 交叉复核。",
  };
}

export async function checkDnsResolvers(
  exit: ExitIpInfo,
  mixedPort?: number | null,
  /** 真实归属参照（由 envRun 实测/推断后传入）；不传时退回系统区域启发式 */
  realCountryOverride?: string | null,
  signal?: AbortSignal,
): Promise<CheckCard> {
  // whoami（主判定）与 Cloudflare loc（次要启发式）并行取：loc 只是 non-leak-proof
  // 的粗对照，绝不参与 verdict，因此单独收紧到 2500ms，缺席时降级为参考文案即可。
  // v0.1.8 之前它串行 await 在主流程里，dev/预览下境外不可达会顶满 6s，把 DNS 卡
  // 拖向 envRun 的 12s deadline（观测到的"超时未响应"根因之一）。
  const [dns, whoami, cf] = await Promise.all([
    listDnsResolvers(),
    dnsWhoami({ timeoutMs: 4500, signal }),
    probeText("https://www.cloudflare.com/cdn-cgi/trace", {
      mixedPort,
      timeoutMs: 2500,
      signal,
    }),
  ]);
  const resolvers = dns.resolvers ?? [];
  // Windows（source 为 win-adapter-*）的 raw_hint 是网卡列表，单独放进 adapterHint，
  // 不拼进 process：评分会在 process 上做「运营商 / 宽带」等关键词匹配（PR #51 M-4）。
  // macOS 的 scutil 原文照旧拼进 process。
  // TODO(后续 PR，与 M3 前端一起)：后端把 TUN 非默认路由等警告做成结构化 warnings 字段，
  // whoami 判「未泄漏」时据此把卡片降为 warn；目前警告只在网卡摘要里。
  const isWinAdapterHint = (dns.source ?? "").startsWith("win-adapter");
  const adapterHint = isWinAdapterHint && dns.rawHint ? dns.rawHint : undefined;

  let loc: string | null = null;
  let colo: string | null = null;
  if (cf.text) {
    for (const line of cf.text.split("\n")) {
      if (line.startsWith("loc=")) loc = line.slice(4).trim();
      if (line.startsWith("colo=")) colo = line.slice(5).trim();
    }
  }
  const exitCc = exit.countryCode?.toUpperCase() ?? null;
  const mismatch =
    exitCc && loc && exitCc !== loc.toUpperCase() && loc.toUpperCase() !== "XX";
  const heuristicLine = mismatch
    ? `启发式粗看（非泄漏鉴定）：出口 ${exitCc} 与 Cloudflare loc=${loc} 不太一致`
    : loc
      ? `启发式粗看（非泄漏鉴定）：出口 ${exitCc ?? "?"}，Cloudflare loc=${loc} · colo=${colo ?? "--"}`
      : "启发式粗看（非泄漏鉴定）：Cloudflare loc 未取到（无代理口或境外不可达），仅供参考";
  let attributionLine: string | null = null;

  const exitIp = exit.ip ?? null;
  const qIp = whoami.clientIp ?? null;

  let level: CheckLevel;
  let conclusion: string;
  let suggestion: string | undefined;

  if (!whoami.ok || !qIp) {
    level = "unknown";
    conclusion = `未能实测 DNS 出口（${whoami.error ?? "DNS 查询无有效返回"}）`;
    suggestion = "检查网络连接是否正常，稍后重试。";
  } else if (!exitIp) {
    level = "warn";
    conclusion = `DNS 查询从 ${qIp} 出去，但缺少代理出口 IP 可对照，无法判定是否泄漏`;
    suggestion = "先完成「出口 IP」检查（或连上代理）后再测 DNS。";
  } else {
    const qCountry = signal?.aborted
      ? null
      : await fetchIpCountry(qIp, mixedPort ?? null, signal);
    const realCountry =
      realCountryOverride !== undefined ? realCountryOverride : localeCountry();
    const v = judgeDnsEgress({ qIp, exitIp, qCountry, realCountry });
    level = v.level;
    conclusion = v.conclusion;
    suggestion = v.suggestion;
    attributionLine = `归属判定：解析出口国家=${qCountry ?? "未知"} · 真实归属参照=${realCountry ?? "未知"}`;
  }

  const proc = [
    `实测：DNS 出口 IP=${qIp ?? "无"} · 递归解析器 ns=${whoami.resolverNs ?? "无"}${
      whoami.ecs ? ` · ECS=${whoami.ecs}` : ""
    }（via ${whoami.via}）`,
    `代理出口 IP=${exitIp ?? "无"}`,
    attributionLine,
    `配置解析器：${resolvers.join(", ") || "(无)"}（source ${dns.source}）`,
    dns.error ? `note: ${dns.error}` : "",
    dns.rawHint && !isWinAdapterHint ? `raw_hint:\n${dns.rawHint}` : "",
    heuristicLine,
    "边界：whoami 反映「查询实际从哪出去」，比读配置更接近真相；ECS 若暴露你的 /24 仍属信息泄露。不是 BrowserLeaks 级全量证明。",
  ].filter(Boolean);

  return {
    id: "dns-leak",
    title: "DNS 解析器",
    level,
    conclusion,
    process: proc.join("\n"),
    suggestion,
    ...(adapterHint ? { adapterHint } : {}),
  };
}

/** @deprecated use checkDnsResolvers — kept name alias for older call sites */
export async function checkDnsLeakApproach(
  exit: ExitIpInfo,
  mixedPort?: number | null,
): Promise<CheckCard> {
  return checkDnsResolvers(exit, mixedPort);
}
