import type { CheckCard } from "./types";

/** Mbps 简写（卡内格子用；去尾随 0，避免 ↓60.0 占位）。 */
function fmtMbps(v: number): string {
  if (!Number.isFinite(v)) return "—";
  const raw =
    v >= 100 ? v.toFixed(0) : v >= 10 ? v.toFixed(1) : v.toFixed(2);
  return raw.replace(/\.0+$/, "").replace(/(\.\d*?)0+$/, "$1");
}

/** 无 metrics 时截短 conclusion，避免长句撑格。 */
function shortConclusion(card: CheckCard, max = 12): string {
  const c = (card.conclusion ?? card.summary ?? "").trim();
  if (!c || c === "尚未检测") return "—";
  if (c === "检测中…") return "检测中";
  if (c.length <= max) return c;
  return `${c.slice(0, max - 1)}…`;
}

/**
 * 节点卡展开格的极简文案：优先 CheckCard.metrics，不把长 conclusion 塞进格子。
 * 完整 conclusion / process / suggestion 留给外置详情面板。
 */
export function formatCardBrief(card: CheckCard): string {
  if (card.level === "running") return "检测中";

  const m = card.metrics;
  const id = card.id;

  if (id === "exit-ip") {
    if (card.level === "fail" && (m?.exitIp == null || m?.exitIp === "")) {
      return "无出口";
    }
    if (m && (m.countryCode || m.hosting != null)) {
      const cc = (m.countryCode ?? "?").toUpperCase();
      if (m.hosting === true) return `${cc} · 机房`;
      // 走到这里 hosting 不是 true。false 只表示「没标成机房」，不能默认家宽。
      // 家宽只在结论已经写明住宅/家宽时沿用那句正面结论。
      const text = card.conclusion ?? "";
      if (/住宅|家宽/.test(text) && !/未判定/.test(text)) {
        return `${cc} · 家宽`;
      }
      return `${cc} · 未判定`;
    }
    return shortConclusion(card);
  }

  if (id === "latency") {
    if (m?.latencyMs != null && Number.isFinite(m.latencyMs)) {
      return `${Math.round(m.latencyMs)} ms`;
    }
    if (card.level === "fail" || m?.latencyMs === null) return "失败";
    return shortConclusion(card);
  }

  if (id === "bandwidth") {
    if (m && (m.downMbps != null || m.upMbps != null)) {
      const d = m.downMbps != null ? fmtMbps(m.downMbps) : "—";
      const u = m.upMbps != null ? fmtMbps(m.upMbps) : "—";
      return `↓${d}\n↑${u}`;
    }
    if (card.level === "fail") return "失败";
    return shortConclusion(card);
  }

  if (id === "reachability") {
    if (card.unverified) return "未验证";
    if (card.level === "pass") return "通";
    if (card.level === "fail") return "不通";
    if (card.level === "warn") return "部分通";
    return shortConclusion(card);
  }

  // 服务解锁汇总卡：全通 / 部分 / 全不通（检测中已在入口处理）
  if (
    id === "svc-streaming" ||
    id === "svc-ai" ||
    id === "svc-store" ||
    id.startsWith("svc-")
  ) {
    if (card.level === "pass") return "全通";
    if (card.level === "warn") return "部分";
    if (card.level === "fail") return "全不通";
    return "未测";
  }

  // 商店：页面区码不是节点商店区，格子只写通或不通。
  if (id === "app-store" || id === "google-play") {
    if (card.level === "pass" || card.level === "warn") return "通";
    if (card.level === "fail") return "不通";
    return shortConclusion(card);
  }

  // 解锁 / 服务类：区码优先，其次开/锁
  if (m && (m.unlockSupported != null || m.unlockRegion || m.unlockLevel)) {
    const region = m.unlockRegion?.trim();
    if (region) {
      if (m.unlockSupported === false) return `锁·${region}`;
      return region;
    }
    if (m.unlockSupported === true) return "开";
    if (m.unlockSupported === false) return "锁";
  }

  return shortConclusion(card);
}
