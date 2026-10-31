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

/**
 * 连接诊断提示（windows-plan §2.7）：根据本次刷新所有候选的发现来源标签展示。
 * InsecureDacl 只要任一来源带 `;pipe-insecure-dacl=` / `pipe-insecure` 就显示，
 * 即使最终选中了别的候选（Ellie PR50 Q5）。macOS 来源不会命中任何提示。
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
  const [stepsOpen, setStepsOpen] = useState(false);
  const notices = sourceNoticesOf(sources);
  if (notices.length === 0) return null;

  const recheck = (
    <button
      type="button"
      className="btn btn-sm"
      disabled={!!busy}
      onClick={onRecheck}
    >
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
              {n.pipes.length > 0 && (
                <div className="diag-notice-detail">
                  {t("diag.insecureDaclPipes", { pipes: n.pipes.join("、") })}
                </div>
              )}
              <div className="diag-notice-actions">
                <button
                  type="button"
                  className="btn btn-primary btn-sm"
                  aria-expanded={stepsOpen}
                  onClick={() => {
                    // 打开 Verge 最新版发布页（Rust 固定地址），同时展开升级步骤与「重新检测」
                    void openVergeReleasePage();
                    setStepsOpen(true);
                  }}
                >
                  {t("diag.insecureDaclAction")}
                </button>
                {stepsOpen && recheck}
              </div>
              {stepsOpen && (
                <ol className="diag-notice-steps">
                  <li>{t("diag.insecureDaclStep1")}</li>
                  <li>{t("diag.insecureDaclStep2")}</li>
                  <li>{t("diag.insecureDaclStep3")}</li>
                </ol>
              )}
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
              <div className="gate-banner-title">{t("diag.untrustedPipe.title")}</div>
              <div className="gate-banner-msg">{t("diag.untrustedPipe.body")}</div>
              <div className="diag-notice-actions">{recheck}</div>
            </div>
          );
        }
        if (n.kind === "tcp-unverified") {
          return (
            <div key={noticeKey(n)} className="gate-banner diag-notice diag-notice-warn" role="status">
              <div className="gate-banner-title">{t("diag.tcpUnverified.title")}</div>
              <div className="gate-banner-msg">{t("diag.tcpUnverified.body")}</div>
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
