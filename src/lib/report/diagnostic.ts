/**
 * 把一次界面上的检测状态收成可发送的错误报告。
 * 自由文本全部经过 sanitize.ts；节点只以索引出现。
 */

import {
  keepSemver,
  keepToken,
  sanitizeText,
  type NameRedaction,
} from "./sanitize";

export type CheckLevel = "pass" | "fail" | "warn" | "unknown";
export type RestoreOutcome = "succeeded" | "failed" | "not_run";

export type ReportPhrases = {
  docTitle: string;
  version: string;
  os: string;
  client: string;
  nodeCount: string;
  restore: string;
  restoreSucceeded: string;
  restoreFailed: string;
  restoreNotRun: string;
  checks: string;
  errors: string;
  nodeLabel: (n: number) => string;
  levelLabel: (level: CheckLevel) => string;
};

export type DiagnosticCheckInput = {
  name: string;
  level: string;
  message?: string;
  nodeIndex?: number;
};

export type DiagnosticInput = {
  version: string;
  os: string;
  arch: string;
  client: string;
  nodeCount: number;
  restoreSucceeded: boolean | null;
  redactions: readonly NameRedaction[];
  checks: readonly DiagnosticCheckInput[];
  errors: readonly { code: string; message: string }[];
};

export type SanitizedCheck = {
  name: string;
  level: CheckLevel;
  message: string;
  nodeIndex?: number;
};

export type SanitizedReport = {
  version: string;
  os: string;
  arch: string;
  client: string;
  nodeCount: number;
  restore: RestoreOutcome;
  checks: SanitizedCheck[];
  errors: { code: string; message: string }[];
  markdown: string;
};

const LEVELS = new Set<CheckLevel>(["pass", "fail", "warn", "unknown"]);

export function normalizeLevel(level: string): CheckLevel | null {
  if (level === "running") return null;
  if (LEVELS.has(level as CheckLevel)) return level as CheckLevel;
  return "unknown";
}

export function restoreOutcome(succeeded: boolean | null): RestoreOutcome {
  if (succeeded === true) return "succeeded";
  if (succeeded === false) return "failed";
  return "not_run";
}

/**
 * 已测节点按列表顺序编成 1、2、3…；没测过的名字若出现在错误文案里，换成 [node]。
 * 名字本身不会写进报告。
 */
export function redactionsForNodes(
  orderedNames: readonly string[],
  testedNames: readonly string[],
  nodeLabel: (n: number) => string,
): { redactions: NameRedaction[]; indexOf: Map<string, number> } {
  const tested = new Set(testedNames.filter((n) => n.length > 0));
  const indexOf = new Map<string, number>();
  const redactions: NameRedaction[] = [];
  let next = 1;
  const assign = (name: string, testedNode: boolean) => {
    if (!name || indexOf.has(name) || redactions.some((r) => r.name === name)) return;
    if (testedNode) {
      const index = next++;
      indexOf.set(name, index);
      redactions.push({ name, replacement: nodeLabel(index) });
    } else {
      redactions.push({ name, replacement: "[node]" });
    }
  };
  for (const name of orderedNames) assign(name, tested.has(name));
  for (const name of testedNames) assign(name, true);
  return { redactions, indexOf };
}

function phraseFor(restore: RestoreOutcome, phrases: ReportPhrases): string {
  if (restore === "succeeded") return phrases.restoreSucceeded;
  if (restore === "failed") return phrases.restoreFailed;
  return phrases.restoreNotRun;
}

export function buildReport(input: DiagnosticInput, phrases: ReportPhrases): SanitizedReport {
  const version = keepSemver(input.version);
  const os = keepToken(input.os);
  const arch = keepToken(input.arch);
  const client = keepToken(input.client);
  const nodeCount = Number.isFinite(input.nodeCount)
    ? Math.max(0, Math.min(100_000, Math.floor(input.nodeCount)))
    : 0;
  const restore = restoreOutcome(input.restoreSucceeded);
  const names = input.redactions;

  const checks: SanitizedCheck[] = [];
  for (const raw of input.checks) {
    const level = normalizeLevel(raw.level);
    if (!level) continue;
    const name = sanitizeText(raw.name, names).trim();
    if (!name) continue;
    const message = sanitizeText(raw.message ?? "", names).replace(/\s+/g, " ").trim();
    const nodeIndex =
      typeof raw.nodeIndex === "number" && raw.nodeIndex >= 1
        ? Math.floor(raw.nodeIndex)
        : undefined;
    checks.push({ name, level, message, nodeIndex });
  }

  const errors = input.errors.map((e) => ({
    code: keepToken(e.code, "error"),
    message: sanitizeText(e.message, names).replace(/\s+/g, " ").trim(),
  }));

  const report: SanitizedReport = {
    version,
    os,
    arch,
    client,
    nodeCount,
    restore,
    checks,
    errors,
    markdown: "",
  };
  report.markdown = renderMarkdown(report, phrases);
  return report;
}

export function renderMarkdown(report: SanitizedReport, phrases: ReportPhrases): string {
  const lines = [
    `# ${phrases.docTitle}`,
    "",
    `- ${phrases.version}: ${report.version}`,
    `- ${phrases.os}: ${report.os} ${report.arch}`,
    `- ${phrases.client}: ${report.client}`,
    `- ${phrases.nodeCount}: ${report.nodeCount}`,
    `- ${phrases.restore}: ${phraseFor(report.restore, phrases)}`,
    "",
    `## ${phrases.checks}`,
  ];
  if (report.checks.length === 0) {
    lines.push("-");
  } else {
    for (const check of report.checks) {
      const who =
        check.nodeIndex != null
          ? `${phrases.nodeLabel(check.nodeIndex)} · ${check.name}`
          : check.name;
      const head = `- ${who}: ${phrases.levelLabel(check.level)}`;
      lines.push(check.message ? `${head} — ${check.message}` : head);
    }
  }
  lines.push("", `## ${phrases.errors}`);
  if (report.errors.length === 0) {
    lines.push("-");
  } else {
    for (const error of report.errors) {
      lines.push(error.message ? `- ${error.code}: ${error.message}` : `- ${error.code}`);
    }
  }
  lines.push("");
  return lines.join("\n");
}

export type CardLike = { name: string; level: string; message?: string };
export type TestedNode = { name: string; cards: readonly CardLike[] };

/** 界面收集到的原始字段 → 送进 buildReport 的输入。节点名只用于编号和清洗。 */
export function assembleInput(args: {
  version: string;
  os: string;
  arch: string;
  client: string;
  nodeNames: readonly string[];
  tested: readonly TestedNode[];
  envCards: readonly CardLike[];
  errors: readonly { code: string; message: string }[];
  restoreSucceeded: boolean | null;
  nodeLabel: (n: number) => string;
}): DiagnosticInput {
  const { redactions, indexOf } = redactionsForNodes(
    args.nodeNames,
    args.tested.map((n) => n.name),
    args.nodeLabel,
  );
  const checks: DiagnosticCheckInput[] = [];
  for (const card of args.envCards) {
    if (card.level === "unknown" || card.level === "running") continue;
    checks.push({ name: card.name, level: card.level, message: card.message });
  }
  for (const node of args.tested) {
    const nodeIndex = indexOf.get(node.name);
    for (const card of node.cards) {
      if (card.level === "unknown" || card.level === "running") continue;
      checks.push({
        name: card.name,
        level: card.level,
        message: card.message,
        nodeIndex,
      });
    }
  }
  return {
    version: args.version,
    os: args.os,
    arch: args.arch,
    client: args.client,
    nodeCount: args.nodeNames.length,
    restoreSucceeded: args.restoreSucceeded,
    redactions,
    checks,
    errors: args.errors,
  };
}
