import { useI18n } from "../i18n";
import { useState } from "react";
import type { CheckCard } from "../lib/egress/types";
import { StatusBadge } from "./StatusBadge";

export function CheckCardView({ card }: { card: CheckCard }) {
  const { t, td } = useI18n();
  const conclusionRaw = card.conclusion ?? card.summary ?? "尚未检测";
  const conclusion = td(conclusionRaw);
  const process = card.process ?? card.detail;
  const suggestion = card.suggestion ?? card.tip;
  const [processOpen, setProcessOpen] = useState(false);
  const hasProcess = !!(process && process.trim());

  return (
    <article className="card card-dense">
      <div className="card-top">
        <h3>{td(card.title)}</h3>
        <StatusBadge level={card.level} conclusion={conclusionRaw} />
      </div>
      <div className="card-conclusion">{conclusion}</div>
      {suggestion ? <div className="card-suggestion">{td(suggestion)}</div> : null}
      {hasProcess ? (
        <>
          <button
            type="button"
            className="card-process-toggle"
            aria-expanded={processOpen}
            onClick={() => setProcessOpen((v) => !v)}
          >
            {processOpen ? t("card.hideProcess") : t("card.showProcess")}
          </button>
          {processOpen ? <div className="card-process">{td(process)}</div> : null}
        </>
      ) : null}
    </article>
  );
}
