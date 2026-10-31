import { useState } from "react";
import { useI18n, type MessageKey } from "../i18n";
import { sourceNoticesOf, type SourceNotice } from "../lib/mihomo";
import { openVergeReleasePage } from "../lib/update";

const REJECT_KEYS: Record<string, MessageKey> = {
  "listener-conflict": "diag.tcpRejected.listenerConflict",
  "token-mismatch": "diag.tcpRejected.tokenMismatch",
  "image-mismatch": "diag.tcpRejected.imageMismatch",
};

function noticeKey(n: SourceNotice): string {
  return n.kind === "tcp-rejected" ? `${n.kind}:${n.code}` : n.kind;
}

/** 提示级别：block = 已拒绝连接（红）；warn = 警告（黄）；info = 说明（中性）。 */
export type NoticeLevel = "block" | "warn" | "info";

export type NoticeView = {
  level: NoticeLevel;
  message: MessageKey;
  vars?: Record<string, string | number>;
};

/** 每类提示的级别与文案（纯函数，便于测试）。 */
export function noticeView(n: SourceNotice): NoticeView {
  switch (n.kind) {
    case "insecure-dacl":
      return { level: "block", message: "diag.insecureDacl" };
    case "untrusted-pipe":
      return { level: "block", message: "diag.untrustedPipe" };
    // S-2（Ellie）：任何本机用户都能经该管道操作控制接口；警告，但不阻止连接
    case "pipe-open-to-all":
      return { level: "warn", message: "diag.pipeOpenToAll" };
    case "tcp-unverified":
      return { level: "warn", message: "diag.tcpUnverified" };
    case "service-mode":
      // 取代 tcp-unverified（连接已被拒绝），所以与它同为警告
      return { level: "warn", message: "diag.serviceMode" };
    case "ipv6-dual-stack":
      // 端口唯一时带端口；值解析失败或多个端口时用通用句
      return n.ports.length === 1
        ? { level: "info", message: "diag.ipv6DualStack", vars: { port: n.ports[0] } }
        : { level: "info", message: "diag.ipv6DualStackGeneric" };
    case "tcp-rejected": {
      const reasonKey = REJECT_KEYS[n.code];
      return reasonKey
        ? { level: "warn", message: reasonKey }
        : { level: "warn", message: "diag.tcpRejected.other", vars: { code: n.code } };
    }
  }
}

const LEVEL_CLASS: Record<NoticeLevel, string> = {
  block: "gate-banner gate-banner-block diag-notice",
  warn: "gate-banner diag-notice diag-notice-warn",
  info: "gate-banner diag-notice diag-notice-info",
};

/**
 * 这条提示是否带「重新检测」按钮。pipe-open-to-all 不带（PR #56 Ellie S-2）：它只是提醒、不影响使用，
 * 重新检测也改变不了结果（要靠升级 Clash Verge）。
 */
export function noticeHasRecheck(n: SourceNotice): boolean {
  return n.kind !== "pipe-open-to-all";
}

/** 打开 Verge 发布页；失败时 setFailed(true)，让按钮旁出一句提示（不吞错误）。 */
export async function openVergeWithFeedback(
  setFailed: (failed: boolean) => void,
  open: () => Promise<void> = openVergeReleasePage,
): Promise<void> {
  setFailed(false);
  try {
    await open();
  } catch {
    setFailed(true);
  }
}

/**
 * 连接诊断提示（windows-plan §2.7）：根据本次刷新所有候选的发现来源标签展示。
 * InsecureDacl 只要任一来源带 `pipe-insecure-dacl` 后缀 / TCP 项 `pipe-insecure` 就显示，
 * 即使最终选中了别的候选（Ellie PR50 Q5）。macOS 来源不会命中任何提示。
 * 文案每条一句话（PR52）：InsecureDacl 只有固定句 + 升级按钮 + 重新检测；pipe-open-to-all 只有一句、不带按钮；
 * 其余提示各一句 + 重新检测。
 * PR #53 新标记：pipe-open-to-all 为警告（不阻止连接），service-mode 为警告（取代 tcp-unverified），ipv6-dual-stack 为说明。
 */
export function DiagnosticsNotices({
  sources,
  busy,
  onRecheck,
}: {
  sources: readonly string[];
  busy?: boolean;
  onRecheck: () => void;
}) {
  const { t } = useI18n();
  const [openFailed, setOpenFailed] = useState(false);
  const notices = sourceNoticesOf(sources);
  if (notices.length === 0) return null;

  const recheck = (
    <button type="button" className="btn btn-sm" disabled={!!busy} onClick={onRecheck}>
      {t("diag.recheck")}
    </button>
  );

  return (
    <div className="diag-notices">
      {notices.map((n) => {
        const view = noticeView(n);
        return (
          <div
            key={noticeKey(n)}
            className={LEVEL_CLASS[view.level]}
            role={view.level === "block" ? "alert" : "status"}
          >
            <div className="gate-banner-msg">{t(view.message, view.vars)}</div>
            <div className="diag-notice-actions">
              {n.kind === "insecure-dacl" ? (
                <button
                  type="button"
                  className="btn btn-primary btn-sm"
                  onClick={() => void openVergeWithFeedback(setOpenFailed)}
                >
                  {t("diag.insecureDaclAction")}
                </button>
              ) : null}
              {noticeHasRecheck(n) ? recheck : null}
              {n.kind === "insecure-dacl" && openFailed ? (
                <span className="ws-error" role="alert">
                  {t("diag.openVergeFailed")}
                </span>
              ) : null}
            </div>
          </div>
        );
      })}
    </div>
  );
}
