import { useEffect, useRef, useState } from "react";
import { useI18n, type MessageKey } from "../i18n";
import {
  browserStore,
  buildReport,
  getOnceViaTauri,
  postOnceViaTauri,
  RATE_LIMITED,
  sendDiagnostic,
  type DiagnosticInput,
  type ReportPhrases,
  type SanitizedReport,
} from "../lib/report";

type Translate = (key: MessageKey, vars?: Record<string, string | number>) => string;

export function reportPhrases(t: Translate): ReportPhrases {
  return {
    docTitle: t("report.docTitle"),
    version: t("report.version"),
    os: t("report.os"),
    client: t("report.client"),
    nodeCount: t("report.nodeCount"),
    restore: t("report.restore"),
    restoreSucceeded: t("report.restoreSucceeded"),
    restoreFailed: t("report.restoreFailed"),
    restoreNotRun: t("report.restoreNotRun"),
    checks: t("report.checks"),
    errors: t("report.errors"),
    nodeLabel: (n) => t("report.nodeLabel", { n }),
    levelLabel: (level) => {
      if (level === "pass") return t("report.levelPass");
      if (level === "fail") return t("report.levelFail");
      if (level === "warn") return t("report.levelWarn");
      return t("report.levelUnknown");
    },
  };
}

export function SendReportButton({ onClick }: { onClick: () => void }) {
  const { t } = useI18n();
  return (
    <button type="button" className="btn btn-sm" onClick={onClick}>
      {t("report.send")}
    </button>
  );
}

/**
 * 出错时先预览再发送。发送失败后预览保持不动，用户可以改联系方式再发一次。
 */
export function useReportActions(input: DiagnosticInput) {
  const { t } = useI18n();
  const phrases = reportPhrases(t);
  const inputRef = useRef(input);
  const phrasesRef = useRef(phrases);
  inputRef.current = input;
  phrasesRef.current = phrases;

  const [open, setOpen] = useState(false);
  const [draft, setDraft] = useState<SanitizedReport | null>(null);
  const [contact, setContact] = useState("");
  const [sending, setSending] = useState(false);
  const [sendError, setSendError] = useState<string | null>(null);
  const [sent, setSent] = useState(false);

  const openSend = () => {
    setDraft(buildReport(inputRef.current, phrasesRef.current));
    setSendError(null);
    setSent(false);
    setSending(false);
    setOpen(true);
  };

  const send = () => {
    if (!draft || sending) return;
    setSending(true);
    setSendError(null);
    void sendDiagnostic(
      {
        version: draft.version,
        os: `${draft.os} ${draft.arch}`,
        client: draft.client,
        report: draft.markdown,
        contact,
      },
      { postOnce: postOnceViaTauri, getOnce: getOnceViaTauri, store: browserStore() },
    )
      .then((result) => {
        if (result.ok) {
          setSent(true);
          return;
        }
        // 失败时留下同一份预览，方便改完联系方式再发。
        if (result.error === RATE_LIMITED) {
          const minutes = Math.max(1, Math.ceil((result.retryAfterMs ?? 60_000) / 60_000));
          setSendError(t("report.rateLimited", { minutes }));
          return;
        }
        setSendError(t("report.sendFailed", { error: result.error ?? "failed" }));
      })
      .finally(() => setSending(false));
  };

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !sending) setOpen(false);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, sending]);

  const dialogs =
    open && draft ? (
      <div className="confirm-overlay">
        <div
          className="confirm-panel card report-panel"
          role="dialog"
          aria-modal="true"
          aria-label={t("report.previewTitle")}
        >
          <p className="confirm-body report-dialog-title">{t("report.previewTitle")}</p>
          <p className="report-hint">{t("report.previewHint")}</p>
          <pre className="report-preview">{draft.markdown}</pre>
          <label className="report-contact-label">
            <span>{t("report.contact")}</span>
            <input
              className="report-contact"
              type="text"
              value={contact}
              maxLength={200}
              autoComplete="off"
              placeholder={t("report.contactPlaceholder")}
              onChange={(e) => setContact(e.target.value)}
            />
          </label>
          {sent ? (
            <p className="report-status report-status-ok" role="status">
              {t("report.sent")}
            </p>
          ) : null}
          {sendError ? (
            <p className="report-status report-status-error" role="alert">
              {sendError}
            </p>
          ) : null}
          <div className="toolbar report-toolbar">
            <button className="btn btn-primary" type="button" disabled={sending} onClick={send}>
              {sending ? t("report.sending") : t("report.sendAction")}
            </button>
            <button className="btn" type="button" disabled={sending} onClick={() => setOpen(false)}>
              {t("report.cancel")}
            </button>
          </div>
        </div>
      </div>
    ) : null;

  return { openSend, dialogs };
}
