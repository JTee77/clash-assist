import { useI18n } from "../i18n";
import type { CheckLevel } from "../lib/egress/types";

const LABELS: Record<CheckLevel, string> = {
  pass: "通过",
  warn: "警告",
  fail: "失败",
  unknown: "未能判定",
  running: "检测中",
};

/** Prefer conclusion-specific labels when level is unknown. */
export function badgeLabel(level: CheckLevel, conclusion?: string | null): string {
  const c0 = (conclusion ?? "").trim();
  if (c0 === "全通" || c0 === "部分" || c0 === "全不通" || c0 === "检测中") {
    return c0;
  }
  if (level === "unknown") {
    const c = (conclusion ?? "").trim();
    if (!c || c === "尚未检测") return "待测";
    if (c.includes("超时未响应") || (/超时/.test(c) && /未响应|无响应/.test(c))) {
      return "超时未响应";
    }
    if (c.includes("未能判定")) return "未能判定";
    return "未能判定";
  }
  return LABELS[level];
}

export function StatusBadge({
  level,
  conclusion,
}: {
  level: CheckLevel;
  conclusion?: string | null;
}) {
  const { td } = useI18n();
  return <span className={`badge ${level}`}>{td(badgeLabel(level, conclusion))}</span>;
}
