import type { CheckCard, CheckLevel } from "../egress/types";
import { SERVICE_IDS } from "./nodeScore";
import type { NodeScoreResult, ScoreBreakdownItem, VpnScoreResult, VpnTier } from "./types";

function levelScore(level: CheckLevel): number {
  switch (level) {
    case "pass":
      return 100;
    case "warn":
      return 55;
    case "fail":
      return 0;
    default:
      return 40;
  }
}

function findCard(cards: CheckCard[], id: string): CheckCard | undefined {
  return cards.find((c) => c.id === id);
}

function tierFromScore(score: number): VpnTier {
  if (score >= 80) return "很好";
  if (score >= 60) return "能用";
  if (score >= 40) return "勉强";
  return "有问题";
}

/** 「DNS 像运营商解析」的关键词。 */
const ISP_DNS_RE = /运营商|ISP DNS|电信|联通|移动|宽带/;

/**
 * 关键词匹配只看 conclusion 与 process（M2 之前就匹配的字段）。
 * Windows 的网卡摘要放在 `adapterHint`，故意不参与：中文 Windows 的 PPPoE 网卡默认叫
 * 「宽带连接」，TUN 模式下它仍在网卡列表里，会把 DNS 分误压到 45（PR #51 M-4）。
 */
function looksLikeIspDns(dns: CheckCard | undefined): boolean {
  if (!dns) return false;
  return ISP_DNS_RE.test(`${dns.conclusion ?? ""} ${dns.process ?? ""}`);
}

/** 彩蛋级「完美」：显式门槛，默认几乎拿不到。 */
export function qualifiesPerfect(
  selected: NodeScoreResult,
  envCards: CheckCard[],
  breakdown: ScoreBreakdownItem[],
  total: number,
): boolean {
  if (selected.stars !== 5 || selected.totalScore < 95) return false;
  if (total < 95) return false;
  if (!breakdown.every((b) => b.score >= 90)) return false;

  const bare =
    findCard(envCards, "bare-egress") ?? findCard(selected.cards, "bare-egress");
  const dns = findCard(envCards, "dns-leak");
  const split = findCard(envCards, "split-routing");
  const ipv6 = findCard(envCards, "ipv6-leak");
  const webrtc = findCard(envCards, "webrtc");

  // 环境卡齐全且过关；缺卡则不能完美
  if (!bare || bare.level !== "pass") return false;
  if (!dns || dns.level !== "pass") return false;
  if (!split || split.level !== "pass") return false;
  if (looksLikeIspDns(dns)) return false;
  if (ipv6 && (ipv6.level === "fail" || ipv6.level === "warn")) return false;
  if (webrtc && (webrtc.level === "fail" || webrtc.level === "warn")) return false;

  const reach = findCard(selected.cards, "reachability");
  if (!reach || reach.level !== "pass") return false;

  const bw = findCard(selected.cards, "bandwidth");
  const down =
    typeof bw?.metrics?.downMbps === "number" && Number.isFinite(bw.metrics.downMbps)
      ? bw.metrics.downMbps
      : null;
  if (down == null || down < 15) return false;

  for (const id of SERVICE_IDS) {
    const c = findCard(selected.cards, id);
    if (!c) continue; // 未跑不强制
    if (c.level !== "pass") return false;
  }

  return true;
}

function mainReason(
  tier: VpnTier,
  tunnel: ScoreBreakdownItem,
  dns: ScoreBreakdownItem,
  split: ScoreBreakdownItem,
  node: ScoreBreakdownItem,
): string {
  const worst = [tunnel, dns, split, node].sort((a, b) => a.score - b.score)[0];
  if (tier === "完美") return "难得一见。几乎挑不出毛病。";
  if (tier === "很好") return "隧道、解析和所选节点都比较顺。";
  if (worst.key === "dns" && dns.score < 60) {
    return "节点还行，但 DNS 仍像运营商解析，容易让人觉得「漏了」。";
  }
  if (worst.key === "tunnel" && tunnel.score < 60) {
    return "节点分数一般，更关键的是隧道本身不够稳。";
  }
  if (worst.key === "split" && split.score < 60) {
    return "出口节点尚可，但分流表现偏怪，国内/国外路径可能不干净。";
  }
  if (worst.key === "node" && node.score < 60) {
    return "整体环境还行，但你选中的这个节点偏弱。";
  }
  if (tier === "能用") return "能正常用，但仍有明显短板，换节点或检查 DNS 会更舒服。";
  if (tier === "勉强") return "勉强能用，建议优先处理得分最低的那一项。";
  return "问题较多，先确认客户端已连上且流量真走代理。";
}

/**
 * 整份 VPN 总评（用户在 App 内选中某个节点结果之后）。
 * 权重：隧道有效 35% · DNS/旁路 25% · 分流 15% · 所选节点可用性 25%。
 * 「完美」为额外显式门槛，不单靠抬高分数线。
 */
export function scoreVpn(
  selected: NodeScoreResult,
  envCards: CheckCard[],
  ranAt = new Date().toISOString(),
): VpnScoreResult {
  const reach = findCard(selected.cards, "reachability");
  const bare =
    findCard(envCards, "bare-egress") ?? findCard(selected.cards, "bare-egress");
  const dns = findCard(envCards, "dns-leak");
  const split = findCard(envCards, "split-routing");

  let tunnelScore = reach ? levelScore(reach.level) : 40;
  let tunnelNote = reach?.conclusion ?? "缺少连通性结果";
  if (bare?.level === "warn" || (bare && /可能未走代理直连|未走代理|直连境外仍通/.test(bare.conclusion))) {
    tunnelScore = Math.min(tunnelScore, 35);
    tunnelNote = bare.conclusion;
  } else if (bare?.level === "pass") {
    tunnelScore = Math.max(tunnelScore, 75);
  }

  const dnsScore = dns ? levelScore(dns.level) : 45;
  const dnsNote = dns?.conclusion ?? "尚未做环境 DNS 检查";
  const ispDns = looksLikeIspDns(dns);
  const dnsAdjusted = ispDns ? Math.min(dnsScore, 45) : dnsScore;

  const splitScore = split ? levelScore(split.level) : 50;
  const splitNote = split?.conclusion ?? "尚未做分流检查";

  const nodeAvail =
    selected.stars === "unavailable"
      ? 0
      : (selected.breakdown.find((b) => b.key === "availability")?.score ??
        selected.totalScore);
  const nodeNote =
    selected.stars === "unavailable"
      ? "所选节点不可用"
      : `${selected.nodeName}：${selected.blurb}`;

  const breakdown: ScoreBreakdownItem[] = [
    { key: "tunnel", label: "隧道是否有效", weight: 0.35, score: tunnelScore, note: tunnelNote },
    { key: "dns", label: "DNS / 旁路", weight: 0.25, score: dnsAdjusted, note: dnsNote },
    { key: "split", label: "分流", weight: 0.15, score: splitScore, note: splitNote },
    { key: "node", label: "所选节点可用性", weight: 0.25, score: nodeAvail, note: nodeNote },
  ];

  const total = Math.round(
    tunnelScore * 0.35 + dnsAdjusted * 0.25 + splitScore * 0.15 + nodeAvail * 0.25,
  );

  let tier = tierFromScore(total);
  if (qualifiesPerfect(selected, envCards, breakdown, total)) {
    tier = "完美";
  }

  return {
    tier,
    totalScore: total,
    reason: mainReason(tier, breakdown[0], breakdown[1], breakdown[2], breakdown[3]),
    breakdown,
    selectedNodeName: selected.nodeName,
    ranAt,
  };
}
