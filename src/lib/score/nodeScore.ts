import type { CheckCard, CheckLevel } from "../egress/types";
import type { NodeScoreResult, NodeStars, ScoreBreakdownItem } from "./types";

function levelToScore(level: CheckLevel): number {
  switch (level) {
    case "pass":
      return 100;
    case "warn":
      return 55;
    case "fail":
      return 0;
    case "running":
      return 0;
    default:
      return 40;
  }
}

function card(cards: CheckCard[], id: string): CheckCard | undefined {
  return cards.find((c) => c.id === id);
}

function scoreAvailability(cards: CheckCard[]): {
  score: number;
  note: string;
  dead: boolean;
  measured: boolean;
} {
  const reach = card(cards, "reachability");
  // 未验证（无代理口且拒绝直连兜底）→ 不计好坏、也不判死，交给权重归一跳过。
  if (reach?.unverified) {
    return { score: 0, note: "未验证：未经代理测试连通性", dead: false, measured: false };
  }
  if (reach?.level === "fail") {
    return {
      score: 0,
      note: "境外探测都失败了，按不可用处理",
      dead: true,
      measured: true,
    };
  }
  if (!reach) return { score: 40, note: "没有连通性结果", dead: false, measured: true };
  return {
    score: levelToScore(reach.level),
    note: reach.conclusion,
    dead: false,
    measured: true,
  };
}

function scoreThroughput(cards: CheckCard[]): {
  score: number;
  note: string;
  measured: boolean;
} {
  const bw = card(cards, "bandwidth");
  if (bw?.unverified) return { score: 0, note: "未验证：未经代理测速", measured: false };
  if (!bw) return { score: 35, note: "未做带宽抽样", measured: true };
  if (bw.level === "fail") return { score: 0, note: bw.conclusion, measured: true };
  const mbps =
    typeof bw.metrics?.downMbps === "number" && Number.isFinite(bw.metrics.downMbps)
      ? bw.metrics.downMbps
      : null;
  if (mbps == null) {
    return { score: levelToScore(bw.level), note: bw.conclusion, measured: true };
  }
  // 文案里的数字取整：≥10 取整，否则 1 位小数（避免 4.745562254259502 Mbps 这类长小数）
  const shown = mbps >= 10 ? Math.round(mbps) : Math.round(mbps * 10) / 10;
  if (mbps >= 40) return { score: 100, note: `下行约 ${shown} Mbps，很充裕`, measured: true };
  if (mbps >= 15) return { score: 88, note: `下行约 ${shown} Mbps，够快`, measured: true };
  if (mbps >= 5) return { score: 72, note: `下行约 ${shown} Mbps，日常能用`, measured: true };
  if (mbps >= 1.5) return { score: 50, note: `下行约 ${shown} Mbps，偏慢`, measured: true };
  if (mbps >= 0.4) {
    return { score: 28, note: `下行约 ${shown} Mbps，明显偏慢`, measured: true };
  }
  return { score: 10, note: `下行约 ${shown} Mbps，几乎不好用`, measured: true };
}

export const SERVICE_IDS = [
  "netflix",
  "disney",
  "youtube",
  "tiktok",
  "spotify",
  "prime-video",
  "app-store",
  "google-play",
  "gemini",
  "chatgpt",
] as const;


function pageLevelPass(c: CheckCard): boolean {
  const text = (c.conclusion ?? "").trim();
  if ((c.id === "tiktok" || c.id === "prime-video") && text.startsWith("可见")) return true;
  if ((c.id === "gemini" || c.id === "chatgpt") && text.startsWith("可用")) return true;
  return false;
}

function scoreServices(cards: CheckCard[]): {
  score: number;
  note: string;
  measured: boolean;
} {
  const list = SERVICE_IDS.map((id) => card(cards, id)).filter(Boolean) as CheckCard[];
  if (!list.length) return { score: 35, note: "未测服务面", measured: true };
  const measured = list.filter((c) => !c.unverified);
  if (!measured.length) {
    return { score: 0, note: "未验证：服务面未经代理测试", measured: false };
  }
  // 页面级通过不加分：TikTok / Prime「可见」，Gemini / ChatGPT「可用」。
  // 它们的「不可用」仍计入。明确不可用按两票，通过按一票。
  const usable = measured.filter((c) => !pageLevelPass(c));
  if (!usable.length) {
    return { score: 35, note: "未测服务面", measured: true };
  }
  let points = 0;
  let votes = 0;
  for (const c of usable) {
    const vote = c.level === "fail" ? 2 : 1;
    points += levelToScore(c.level) * vote;
    votes += vote;
  }
  const avg = points / votes;
  const passN = usable.filter((c) => c.level === "pass").length;
  const failN = usable.filter((c) => c.level === "fail").length;
  return {
    score: Math.round(avg),
    note: `${passN} 项较顺、${failN} 项不行（共 ${usable.length} 项）`,
    measured: true,
  };
}

function scoreExit(cards: CheckCard[]): {
  score: number;
  note: string;
  measured: boolean;
} {
  const exit = card(cards, "exit-ip");
  if (exit?.unverified) return { score: 0, note: "未验证：未取得代理出口信息", measured: false };
  if (!exit) return { score: 40, note: "未拿到出口信息", measured: true };
  if (exit.level === "fail") return { score: 0, note: exit.conclusion, measured: true };
  const hostingMetric = exit.metrics?.hosting;
  const copy = `${exit.conclusion} ${exit.process ?? ""}`;
  const hosting =
    hostingMetric === true ||
    (hostingMetric == null && /机房|DCH|hosting/i.test(copy));
  const residential =
    hostingMetric === false ||
    (hostingMetric == null && /住宅|ISP|家宽/i.test(copy));
  if (exit.level === "pass" && residential && !hosting) {
    return { score: 95, note: "出口更像住宅线路", measured: true };
  }
  if (hosting) {
    return { score: 55, note: "出口更像机房地址，有的网站会更挑剔", measured: true };
  }
  return { score: levelToScore(exit.level), note: exit.conclusion, measured: true };
}

function mapStars(total: number, dead: boolean): NodeStars {
  if (dead || total <= 0) return "unavailable";
  if (total >= 92) return 5;
  if (total >= 85) return 4.5;
  if (total >= 78) return 4;
  if (total >= 70) return 3.5;
  if (total >= 62) return 3;
  if (total >= 52) return 2.5;
  if (total >= 42) return 2;
  if (total >= 30) return 1.5;
  return 1;
}

function blurbFor(
  stars: NodeStars,
  thrNote: string,
  availNote: string,
  svcNote: string,
  exitNote: string,
): string {
  if (stars === "unavailable") {
    return "连海外站点都打不开，先别指望用它上网。";
  }
  if (stars === 5) return `表现很稳：${thrNote}；服务面也较齐。`;
  if (stars === 4.5) return `接近满分：${thrNote}；服务面大体齐。`;
  if (stars === 4) return `整体不错：${thrNote}。${svcNote}。`;
  if (stars === 3.5) return `比较能打：${thrNote}；个别项还可更好。`;
  if (stars === 3) return `能用，但不算出众：${thrNote}。`;
  if (stars === 2.5) return `勉强够用：${thrNote}；体验一般。`;
  if (stars === 2) return `勉强能用：${thrNote}；${exitNote}`;
  if (stars === 1.5) return `偏弱，偶尔能通：${availNote}；${thrNote}`;
  return `偏弱：${availNote}；${thrNote}`;
}

/** Numeric rank for sorting; unavailable sorts last via -1. */
export function starRank(stars: NodeStars): number {
  return stars === "unavailable" ? -1 : stars;
}

function fakeLowLatencyTip(cards: CheckCard[], thrScore: number): string | undefined {
  const lat = card(cards, "latency");
  const bw = card(cards, "bandwidth");
  const mbps =
    typeof bw?.metrics?.downMbps === "number" && Number.isFinite(bw.metrics.downMbps)
      ? bw.metrics.downMbps
      : null;
  if (!lat || lat.level === "fail") return undefined;
  const ms =
    typeof lat.metrics?.latencyMs === "number" && Number.isFinite(lat.metrics.latencyMs)
      ? lat.metrics.latencyMs
      : (() => {
          const m = lat.conclusion.match(/(\d+)\s*ms/);
          return m ? Number(m[1]) : null;
        })();
  if (ms != null && ms < 120 && (thrScore < 40 || (mbps != null && mbps < 0.8))) {
    return "延迟看起来不高，但实际下载偏慢，有可能是「假低延迟」。选节点时别只看延迟数字。";
  }
  return undefined;
}

/**
 * 按检测卡片给节点打星（不跨次比较）。
 * 权重：可用性 25% · 吞吐 30% · 服务 30% · 出口质量 15%。
 * 延迟不进主权重。
 */
export function scoreNodeFromCards(
  nodeName: string,
  cards: CheckCard[],
  ranAt = new Date().toISOString(),
): NodeScoreResult {
  const avail = scoreAvailability(cards);
  if (avail.dead) {
    const breakdown: ScoreBreakdownItem[] = [
      { key: "availability", label: "可用性", weight: 0.25, score: 0, note: avail.note },
      { key: "throughput", label: "吞吐抽样", weight: 0.3, score: 0, note: "未测或不可用" },
      { key: "services", label: "服务面", weight: 0.3, score: 0, note: "未测或不可用" },
      { key: "exit", label: "出口质量", weight: 0.15, score: 0, note: "未测或不可用" },
    ];
    return {
      nodeName,
      stars: "unavailable",
      totalScore: 0,
      blurb: "连海外站点都打不开，先别指望用它上网。",
      breakdown,
      cards,
      ranAt,
    };
  }

  const thr = scoreThroughput(cards);
  const svc = scoreServices(cards);
  const exit = scoreExit(cards);

  // 四个维度各自带 measured 标记；未验证的维度不计好坏、也不拖星——按剩余权重归一。
  type Dim = {
    key: string;
    label: string;
    weight: number;
    score: number;
    note: string;
    measured: boolean;
  };
  const dims: Dim[] = [
    { key: "availability", label: "可用性", weight: 0.25, score: avail.score, note: avail.note, measured: avail.measured },
    { key: "throughput", label: "吞吐抽样", weight: 0.3, score: thr.score, note: thr.note, measured: thr.measured },
    { key: "services", label: "服务面", weight: 0.3, score: svc.score, note: svc.note, measured: svc.measured },
    { key: "exit", label: "出口质量", weight: 0.15, score: exit.score, note: exit.note, measured: exit.measured },
  ];
  const measured = dims.filter((d) => d.measured);
  const measuredWeight = measured.reduce((s, d) => s + d.weight, 0);

  const breakdown: ScoreBreakdownItem[] = dims.map((d) => ({
    key: d.key,
    label: d.label,
    weight: d.weight,
    score: d.score,
    note: d.measured ? d.note : `未验证 · 不计入评分（${d.note}）`,
  }));

  // 全部维度都未验证（例如压根没有代理口）→ 不评分，也不误判成「死节点」。
  if (measuredWeight <= 0) {
    return {
      nodeName,
      stars: "unavailable",
      totalScore: 0,
      blurb: "未取到代理口 / 各项均未经代理验证，暂无法评分。",
      breakdown,
      cards,
      ranAt,
    };
  }

  const total = Math.round(
    measured.reduce((s, d) => s + d.score * d.weight, 0) / measuredWeight,
  );
  const stars = mapStars(total, false);

  return {
    nodeName,
    stars,
    totalScore: total,
    blurb: blurbFor(stars, thr.note, avail.note, svc.note, exit.note),
    breakdown,
    fakeLowLatencyTip: fakeLowLatencyTip(cards, thr.score),
    cards,
    ranAt,
  };
}

/** 仅延迟探测失败 → 不可用（用于「测全部」淘汰） */
export function scoreDeadNode(nodeName: string, reason: string): NodeScoreResult {
  return {
    nodeName,
    stars: "unavailable",
    totalScore: 0,
    blurb: reason || "延迟探测失败，按不可用处理。",
    breakdown: [
      { key: "availability", label: "可用性", weight: 0.25, score: 0, note: reason },
      { key: "throughput", label: "吞吐抽样", weight: 0.3, score: 0, note: "未检测" },
      { key: "services", label: "服务面", weight: 0.3, score: 0, note: "未检测" },
      { key: "exit", label: "出口质量", weight: 0.15, score: 0, note: "未检测" },
    ],
    cards: [],
    ranAt: new Date().toISOString(),
  };
}

/** Plain-text label for a11y / logs; UI should use <StarRating>. */
export function formatStars(stars: NodeStars): string {
  if (stars === "unavailable") return "不可用";
  return `${stars} 星`;
}
