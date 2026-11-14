import { useEffect, useRef, useState } from "react";
import { useI18n, type MessageKey } from "../i18n";
import {
  browserStore,
  buildReport,
  postOnceViaTauri,
  RATE_LIMITED,
  renderSharePngBase64,
  savePngDocument,
  saveTextDocument,
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
    disclaimer: t("report.disclaimer"),
    nodeLabel: (n) => t("report.nodeLabel", { n }),
    levelLabel: (level) => {
      if (level === "pass") return t("report.levelPass");
      if (level === "fail") return t("report.levelFail");
      if (level === "warn") return t("report.levelWarn");
      return t("report.levelUnknown");
    },
    moreNodes: (n) => t("report.moreNodes", { n }),
  };
}

function errText(err: unknown): string {
  if (typeof err === "string" && err.trim()) return err.trim();
  if (err instanceof Error && err.message) return err.message;
  return "failed";
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
 * 导出 / 分享，以及出错时的预览发送。
 * 预览里的报告在发送失败后保持不动，用户可以改联系方式再发一次。
 */
export function useReportActions(input: DiagnosticInput, canShare: boolean) {
  const { t } = useI18n();
  const phrases = reportPhrases(t);
  const inputRef = useRef(input);
  const phrasesRef = useRef(phrases);
  inputRef.current = input;
  phrasesRef.current = phrases;

  const [mode, setMode] = useState<null | "send" | "share">(null);
  const [draft, setDraft] = useState<SanitizedReport | null>(null);
  const [contact, setContact] = useState("");
  const [sending, setSending] = useState(false);
  const [sendError, setSendError] = useState<string | null>(null);
  const [sent, setSent] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);

  const openSend = () => {
    setDraft(buildReport(inputRef.current, phrasesRef.current));
    setSendError(null);
    setSent(false);
    setSending(false);
    setMode("send");
  };

  const openShare = () => {
    setDraft(buildReport(inputRef.current, phrasesRef.current));
    setSendError(null);
    setMode("share");
  };

  const exportReport = () => {
    const report = buildReport(inputRef.current, phrasesRef.current);
    setNotice(null);
    void saveTextDocument("Clash-Assist-diagnostic.md", report.markdown, "md")
      .then((saved) => {
        if (saved) setNotice(t("report.saved"));
      })
      .catch((err: unknown) => {
        setNotice(t("report.saveFailed", { error: errText(err) }));
      });
  };

  const saveShare = () => {
    if (!draft) return;
    const text = draft.shareLines.join("\n");
    setSendError(null);
    let png = "";
    try {
      png = renderSharePngBase64(draft.shareLines);
    } catch (err: unknown) {
      setSendError(t("report.saveFailed", { error: errText(err) }));
      return;
    }
    void savePngDocument("Clash-Assist-result.png", png, text)
      .then((saved) => {
        if (!saved) return;
        setNotice(
          saved.sidecar === "kept" ? t("report.shareSavedImageOnly") : t("report.shareSaved"),
        );
      })
      .catch((err: unknown) => {
        setSendError(t("report.saveFailed", { error: errText(err) }));
      });
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
      { postOnce: postOnceViaTauri, store: browserStore() },
    )
      .then((result) => {
        if (result.ok) {
          setSent(true);
          return;
        }
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
    if (!mode) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !sending) setMode(null);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [mode, sending]);

  const bar = (
    <div className="home-report-row">
      <button type="button" className="btn btn-sm" onClick={exportReport}>
        {t("report.export")}
      </button>
      <button type="button" className="btn btn-sm" disabled={!canShare} onClick={openShare}>
        {t("report.share")}
      </button>
      {notice ? (
        <span className="report-notice" role="status">
          {notice}
        </span>
      ) : null}
    </div>
  );

  const dialogs =
    mode && draft ? (
      <div className="confirm-overlay">
        <div
          className="confirm-panel card report-panel"
          role="dialog"
          aria-modal="true"
          aria-label={mode === "send" ? t("report.previewTitle") : t("report.shareTitle")}
        >
          <p className="confirm-body report-dialog-title">
            {mode === "send" ? t("report.previewTitle") : t("report.shareTitle")}
          </p>
          {mode === "send" ? <p className="report-hint">{t("report.previewHint")}</p> : null}
          <pre className="report-preview">
            {mode === "send" ? draft.markdown : draft.shareLines.join("\n")}
          </pre>
          {mode === "send" ? (
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
          ) : null}
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
            {mode === "send" ? (
              <button
                className="btn btn-primary"
                type="button"
                disabled={sending}
                onClick={send}
              >
                {sending ? t("report.sending") : t("report.sendAction")}
              </button>
            ) : (
              <button className="btn btn-primary" type="button" onClick={saveShare}>
                {t("report.saveImage")}
              </button>
            )}
            <button
              className="btn"
              type="button"
              disabled={sending}
              onClick={() => setMode(null)}
            >
              {t("report.cancel")}
            </button>
          </div>
        </div>
      </div>
    ) : null;

  return { openSend, bar, dialogs };
}
