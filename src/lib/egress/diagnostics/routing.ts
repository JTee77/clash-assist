//! 分流抽检（国内/境外经代理 vs 直连 + 规则库抽样）与直连旁路粗检。
import { fetchTextViaProxy } from "../fetchVia";
import { getRulesSummary } from "../../mihomo/client";
import type { ControllerConfig } from "../../mihomo/types";
import { isReachableStatus, UA } from "./probe";
import type { CheckCard, CheckLevel } from "../types";


type SampleProbe = {
  label: string;
  host: string;
  url: string;
  kind: "cn" | "foreign";
  ok: boolean;
  status: number;
  ms: number;
  via: string;
};

async function probeSampleHost(
  label: string,
  host: string,
  url: string,
  kind: "cn" | "foreign",
  mixedPort: number | null,
  timeoutMs: number,
  signal?: AbortSignal,
): Promise<SampleProbe> {
  const t0 = performance.now();
  const r = await fetchTextViaProxy(url, {
    mixedPort,
    signal,
    timeoutMs,
    userAgent: UA,
  });
  const ms = Math.round(performance.now() - t0);
  const ok =
    r.status === 204 ||
    r.status === 200 ||
    (r.status >= 200 && r.status < 400) ||
    (r.ok && r.status !== 0);
  return {
    label,
    host,
    url,
    kind,
    ok,
    status: r.status,
    ms,
    via: r.via + (mixedPort != null && mixedPort > 0 ? `+mixed:${mixedPort}` : "+direct"),
  };
}

/**
 * B5: 分流抽检 — domestic tend DIRECT / foreign via proxy (sample, not full audit).
 */
export async function checkSplitRouting(
  mixedPort?: number | null,
  mihomoConfig?: ControllerConfig | null,
  signal?: AbortSignal,
): Promise<CheckCard> {
  const port = mixedPort ?? null;
  const expectProxy = port != null && port > 0;
  const timeoutMs = 4000;

  const cnTargets: { label: string; host: string; url: string }[] = [
    { label: "百度", host: "www.baidu.com", url: "https://www.baidu.com/" },
    { label: "腾讯", host: "www.qq.com", url: "https://www.qq.com/" },
  ];
  const foreignTargets: { label: string; host: string; url: string }[] = [
    {
      label: "Google 204",
      host: "www.google.com",
      url: "https://www.google.com/generate_204",
    },
    {
      label: "YouTube",
      host: "www.youtube.com",
      url: "https://www.youtube.com/",
    },
  ];

  const cnViaMixed: SampleProbe[] = [];
  const foreignViaMixed: SampleProbe[] = [];
  const cnDirect: SampleProbe[] = [];

  if (expectProxy) {
    for (const t of cnTargets) {
      cnViaMixed.push(
        await probeSampleHost(t.label, t.host, t.url, "cn", port, timeoutMs, signal),
      );
    }
    for (const t of foreignTargets) {
      foreignViaMixed.push(
        await probeSampleHost(t.label, t.host, t.url, "foreign", port, timeoutMs, signal),
      );
    }
  }

  // Direct CN baseline (true no-proxy) for rough DIRECT tendency
  for (const t of cnTargets) {
    cnDirect.push(
      await probeSampleHost(t.label, t.host, t.url, "cn", null, timeoutMs, signal),
    );
  }

  let rulesLine = "规则库：未查询（无 Mihomo 配置或不适用）";
  if (mihomoConfig) {
    const rules = await getRulesSummary(mihomoConfig);
    if (rules.error) {
      rulesLine = `规则库：${rules.error}`;
    } else {
      rulesLine = `规则库抽样：共 ${rules.total} 条 · DIRECT ${rules.directCount} · REJECT ${rules.rejectCount} · 其它 ${rules.otherCount} · 含 CN/国内线索 ${rules.cnHintCount}`;
      if (rules.samples.length) {
        rulesLine += `\n  例：${rules.samples.slice(0, 4).join("； ")}`;
      }
    }
  }

  const fmt = (p: SampleProbe) =>
    `${p.label}(${p.host}) → ${p.ok ? "可达" : "失败"} HTTP ${p.status || "超时"} · ${p.ms}ms · ${p.via}`;

  const cnOk = cnViaMixed.filter((p) => p.ok).length;
  const foreignOk = foreignViaMixed.filter((p) => p.ok).length;
  const cnDirectOk = cnDirect.filter((p) => p.ok).length;

  // Latency heuristic: CN via mixed ≈ CN direct → likely DIRECT for those samples
  let latencyHint = "延迟对照：跳过（无 mixed-port 对照）";
  if (expectProxy && cnViaMixed.length && cnDirect.length) {
    const mixedAvg =
      cnViaMixed.reduce((s, p) => s + p.ms, 0) / Math.max(1, cnViaMixed.length);
    const directAvg =
      cnDirect.reduce((s, p) => s + p.ms, 0) / Math.max(1, cnDirect.length);
    const ratio = directAvg > 0 ? mixedAvg / directAvg : 0;
    if (cnOk > 0 && cnDirectOk > 0 && ratio > 0 && ratio < 2.2) {
      latencyHint = `延迟对照：国内经 mixed 均 ${Math.round(mixedAvg)}ms ≈ 直连 ${Math.round(directAvg)}ms（比值 ${ratio.toFixed(2)}）— 更像走 DIRECT`;
    } else if (cnOk > 0 && cnDirectOk > 0 && ratio >= 2.2) {
      latencyHint = `延迟对照：国内经 mixed 均 ${Math.round(mixedAvg)}ms 明显高于直连 ${Math.round(directAvg)}ms（比值 ${ratio.toFixed(2)}）— 可能被代理绕行`;
    } else {
      latencyHint = `延迟对照：国内 mixed 均 ${Math.round(mixedAvg)}ms · 直连 ${Math.round(directAvg)}ms（样本不足或失败，仅供参考）`;
    }
  }

  const process = [
    expectProxy
      ? `经 mixed-port(${port}) 国内：${cnOk}/${cnViaMixed.length} 可达`
      : "未配置 mixed-port，跳过「经代理」国内/境外对照",
    expectProxy
      ? `经 mixed-port(${port}) 境外：${foreignOk}/${foreignViaMixed.length} 可达`
      : "",
    `直连国内基线：${cnDirectOk}/${cnDirect.length} 可达`,
    latencyHint,
    rulesLine,
    "—— 样本明细 ——",
    ...(expectProxy ? cnViaMixed.map((p) => `国内·代理 ${fmt(p)}`) : []),
    ...(expectProxy ? foreignViaMixed.map((p) => `境外·代理 ${fmt(p)}`) : []),
    ...cnDirect.map((p) => `国内·直连 ${fmt(p)}`),
    "测了什么：少量国内/境外域名 HTTPS 抽样 + 可选 /rules 计数。",
    "没测：完整规则表逐条匹配、GEOIP 数据库正确性、UDP/QUIC、所有订阅域名。",
    "边界：这是「分流检查」不是完整规则审计；结果随节点与规则集变化。",
  ]
    .filter(Boolean)
    .join("\n");

  if (!expectProxy) {
    return {
      id: "split-routing",
      title: "分流检查",
      level: cnDirectOk > 0 ? "unknown" : "fail",
      conclusion: cnDirectOk > 0
        ? "仅完成直连国内基线；尚未连上代理口，无法判断分流"
        : "国内直连基线失败，且尚未连上代理口",
      process,
      suggestion: "请先点「刷新节点」确保已连上软件，再重测分流。",
    };
  }

  let level: CheckLevel;
  let conclusion: string;
  if (cnOk === 0 && foreignOk === 0) {
    level = "fail";
    conclusion = "经代理的国内与境外样本均失败";
  } else if (cnOk === cnViaMixed.length && foreignOk === foreignViaMixed.length) {
    level = "pass";
    conclusion = `抽样正常：国内 ${cnOk}/${cnViaMixed.length} · 境外 ${foreignOk}/${foreignViaMixed.length} 经代理可达`;
  } else if (cnOk > 0 && foreignOk === 0) {
    level = "warn";
    conclusion = `国内可达但境外经代理失败（${foreignOk}/${foreignViaMixed.length}）— 代理路径或规则可能异常`;
  } else if (cnOk === 0 && foreignOk > 0) {
    level = "warn";
    conclusion = `境外可达但国内经代理失败 — 国内站或 DIRECT 规则可能异常`;
  } else {
    level = "warn";
    conclusion = `部分可达：国内 ${cnOk}/${cnViaMixed.length} · 境外 ${foreignOk}/${foreignViaMixed.length}`;
  }

  return {
    id: "split-routing",
    title: "分流检查",
    level,
    conclusion,
    process,
    suggestion: undefined,
  };
}

/**
 * B6: 直连旁路粗检 — mixed-port fail + direct foreign OK → 可能未走代理直连.
 */
export async function checkBareEgress(
  mixedPort?: number | null,
  signal?: AbortSignal,
): Promise<CheckCard> {
  const port = mixedPort ?? null;
  const expectProxy = port != null && port > 0;
  const timeoutMs = 4000;
  const targets = [
    {
      label: "Google 204",
      url: "https://www.google.com/generate_204",
    },
    {
      label: "Cloudflare 204",
      url: "https://cp.cloudflare.com/generate_204",
    },
  ];

  async function one(
    url: string,
    label: string,
    mp: number | null,
  ): Promise<{ label: string; url: string; ok: boolean; status: number; ms: number; path: string }> {
    const t0 = performance.now();
    if (signal?.aborted) {
      return { label, url, ok: false, status: 0, ms: 0, path: "已取消" };
    }
    const r = await fetchTextViaProxy(url, {
      mixedPort: mp,
      timeoutMs,
      signal,
    });
    const ms = Math.round(performance.now() - t0);
    const ok = isReachableStatus(r.status, r.ok) || (r.status >= 200 && r.status < 400);
    return {
      label,
      url,
      ok,
      status: r.status,
      ms,
      path: mp != null && mp > 0 ? `mixed-port(${mp})` : "直连(无代理)",
    };
  }

  const viaMixed = [];
  const viaDirect = [];
  for (const t of targets) {
    if (expectProxy) {
      viaMixed.push(await one(t.url, t.label, port));
      if (viaMixed[viaMixed.length - 1].ok) break;
    }
  }
  for (const t of targets) {
    viaDirect.push(await one(t.url, t.label, null));
    if (viaDirect[viaDirect.length - 1].ok) break;
  }

  const mixedOk = viaMixed.some((p) => p.ok);
  const directOk = viaDirect.some((p) => p.ok);

  const lines = [
    expectProxy
      ? `经代理：${mixedOk ? "可达" : "失败"}（探测 ${viaMixed.length} 次）`
      : "未配置 mixed-port，无法做「代理应在线」对照",
    `直连境外：${directOk ? "可达" : "失败"}（探测 ${viaDirect.length} 次）`,
    ...viaMixed.map(
      (p) => `代理 ${p.label} → ${p.ok ? "OK" : "失败"} HTTP ${p.status || "超时"} · ${p.ms}ms · ${p.path}`,
    ),
    ...viaDirect.map(
      (p) => `直连 ${p.label} → ${p.ok ? "OK" : "失败"} HTTP ${p.status || "超时"} · ${p.ms}ms · ${p.path}`,
    ),
    "测了什么：同一境外 HTTPS 探测点，分别走 mixed-port 与 Rust 真直连（不走系统代理）。",
    "没测：TUN 是否真正接管、系统代理开关、各 App 是否各自走代理、防火墙状态。",
    "边界：本应用无法单独从进程内完整获知 Clash TUN 内核状态；「未走代理直连」仅为抽样告警。",
  ];

  if (!expectProxy) {
    return {
      id: "bare-egress",
      title: "直连旁路检查",
      level: "unknown",
      conclusion: directOk
        ? "直连境外可达；尚未连上代理口，无法判断是否存在未走代理直连"
        : "直连境外不可达；尚未连上代理口",
      process: lines.join("\n"),
      suggestion: undefined,
    };
  }

  let level: CheckLevel;
  let conclusion: string;
  if (!mixedOk && directOk) {
    level = "warn";
    conclusion = "可能未走代理直连：代理路径失败但直连境外仍通";
  } else if (!mixedOk && !directOk) {
    level = "unknown";
    conclusion = "代理与直连境外均失败 — 可能离线或探测点不可达";
  } else if (mixedOk && !directOk) {
    level = "pass";
    conclusion = "代理路径可达，直连境外失败 — 未见「代理挂了仍直出」";
  } else {
    // both OK
    level = "pass";
    conclusion = "代理路径可达（直连境外亦通，属环境常见情况）";
  }

  return {
    id: "bare-egress",
    title: "直连旁路检查",
    level,
    conclusion,
    process: lines.join("\n"),
    suggestion: undefined,
  };
}
