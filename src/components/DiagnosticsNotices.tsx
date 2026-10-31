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
 * 文案每条一句话（PR52）：InsecureDacl 只有固定句 + 升级按钮 + 重新检测。
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
        if (n.kind === "insecure-dacl") {
          return (
            <div
              key={noticeKey(n)}
              className="gate-banner gate-banner-block diag-notice"
              role="alert"
            >
              <div className="gate-banner-msg">{t("diag.insecureDacl")}</div>
              <div className="diag-notice-actions">
                <button
                  type="button"
                  className="btn btn-primary btn-sm"
                  onClick={() => void openVergeWithFeedback(setOpenFailed)}
                >
                  {t("diag.insecureDaclAction")}
                </button>
                {recheck}
                {openFailed ? (
                  <span className="ws-error" role="alert">
                    {t("diag.openVergeFailed")}
                  </span>
                ) : null}
              </div>
            </div>
          );
        }
        if (n.kind === "untrusted-pipe") {
          return (
            <div
              key={noticeKey(n)}
              className="gate-banner gate-banner-block diag-notice"
              role="alert"
            >
              <div className="gate-banner-msg">{t("diag.untrustedPipe")}</div>
              <div className="diag-notice-actions">{recheck}</div>
            </div>
          );
        }
        if (n.kind === "tcp-unverified") {
          return (
            <div key={noticeKey(n)} className="gate-banner diag-notice diag-notice-warn" role="status">
              <div className="gate-banner-msg">{t("diag.tcpUnverified")}</div>
              <div className="diag-notice-actions">{recheck}</div>
            </div>
          );
        }
        const reasonKey = REJECT_KEYS[n.code];
        return (
          <div key={noticeKey(n)} className="gate-banner diag-notice diag-notice-warn" role="status">
            <div className="gate-banner-title">{t("diag.tcpRejected.title")}</div>
            <div className="gate-banner-msg">
              {reasonKey ? t(reasonKey) : t("diag.tcpRejected.other", { code: n.code })}{" "}
              {t("diag.tcpRejected.guide")}
            </div>
            <div className="diag-notice-actions">{recheck}</div>
          </div>
        );
      })}
    </div>
  );
}
