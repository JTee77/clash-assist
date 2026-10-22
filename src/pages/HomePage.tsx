import { useEffect, useState } from "react";
import { CheckCardView } from "../components/CheckCardView";
import { ThemeToggle } from "../components/ThemeToggle";
import { LanguageToggle } from "../components/LanguageToggle";
import { useI18n } from "../i18n";
import { NodeCard } from "../components/NodeCard";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  CLIENT_OPTIONS,
  normalizeClientId,
  shortRegionName,
  type ClientId,
  type ConnectionState,
  type ProxyNode,
} from "../lib/mihomo";
import { useNodeFilter } from "../hooks/useNodeFilter";
import {
  splitProgressDisplay,
  useRunnerController,
} from "../hooks/useRunnerController";

export { splitProgressDisplay };

export function HomePage({
  connection,
  busy,
  nodes,
  clientId,
  onClientIdChange,
  onRefresh,
  forceMock = false,
}: {
  connection: ConnectionState;
  busy?: boolean;
  nodes: ProxyNode[];
  clientId: ClientId | null;
  onClientIdChange: (id: ClientId) => void;
  onRefresh?: (explicitClientId?: ClientId | null) => Promise<unknown> | void;
  forceMock?: boolean;
}) {
  const { t, td } = useI18n();
  const [copiedDonate, setCopiedDonate] = useState(false);

  const clientUnset = !clientId;

  const {
    cacheScope,
    mixedPortNum,
    nodeCards,
    envCards,
    running,
    refreshing,
    mode,
    gate,
    nodeScores,
    envOpen,
    envRunning,
    envCheckOk,
    nodeTestOk,
    allConfirmOpen,
    setAllConfirmOpen,
    switchHint,
    restoreError,
    testingNode,
    progressDisplay,
    barPct,
    cachedEnvTimeText,
    cachedNodesTimeText,
    refreshAndGate,
    runEnvCheck,
    openConfirmAll,
    confirmAll,
    onAbortAll,
    testOneNode,
  } = useRunnerController({
    connection,
    nodes,
    clientId,
    forceMock,
    onRefresh,
  });

  const {
    regionFilter,
    setRegionFilter,
    searchQuery,
    setSearchQuery,
    resetFilter,
    expanded,
    allExpanded,
    toggleExpand,
    expandNode,
    toggleExpandAll,
    scoreByName,
    orderedNodes,
    regionGroups,
    isFiltered,
    filteredNodes,
    inlineRegions,
    overflowRegions,
    overflowNodeCount,
    overflowActive,
    nodeColumns,
  } = useNodeFilter({
    nodes,
    currentProxy: connection.currentProxy,
    nodeScores,
    cacheScope,
  });

  const onSelectClient = (raw: string) => {
    const picked = CLIENT_OPTIONS.find((o) => o.id === raw);
    if (!picked || picked.disabled) return;
    const id = normalizeClientId(raw);
    if (!id) return;
    onClientIdChange(id);
    // 手动切换软件后立即自动拉取该软件的节点并校验门禁
    void refreshAndGate(id);
  };

  // v0.1.10：版本号进原生标题栏（玻璃窗口下正文不再放介绍卡）。
  useEffect(() => {
    if (typeof window !== "undefined" && "__TAURI_INTERNALS__" in window) {
      void getCurrentWindow()
        .setTitle(`${t("appName")} ${__APP_VERSION__}`)
        .catch(() => {});
    }
  }, [t]);

  return (
    <div className="home-page">
      <header className="home-topbar">
        <div className="client-ops-cluster">
          <div className="client-field">
            <label
              className="client-picker-label home-block-title"
              htmlFor="home-client-select"
            >
              {t("home.whichApp")}
            </label>
            <select
              id="home-client-select"
              className="client-picker-select"
              value={clientId ?? ""}
              onChange={(e) => onSelectClient(e.target.value)}
              title={
                clientUnset
                  ? t("home.pickFirstTitle")
                  : td(CLIENT_OPTIONS.find((o) => o.id === clientId)?.hint)
              }
            >
              <option value="" disabled>
                {t("home.pleaseSelect")}
              </option>
              {CLIENT_OPTIONS.map((o) => (
                <option key={o.id} value={o.id} disabled={o.disabled}>
                  {td(o.label)}
                </option>
              ))}
            </select>
          </div>

          <div className="action-with-ok">
            <button
              className="btn btn-primary action-btn"
              type="button"
              disabled={!!busy || refreshing || running}
              onClick={() => void refreshAndGate()}
            >
              {busy || refreshing ? t("home.fetching") : t("home.getNodes")}
            </button>
            {!refreshing &&
            (connection.status === "connected" && gate?.ok ? (
              <span className="fetch-ok" title={td(connection.message)}>
                <span className="status-ok-mark">✓</span>
                {t("home.syncedNodes", { n: orderedNodes.length })}
              </span>
            ) : gate && !gate.ok ? (
              <>
                <span className="fetch-fail">
                  <span className="status-fail-mark">✗</span>
                  {t("home.failed")}
                </span>
                <span className="fetch-fail-msg" role="status">
                  {td(gate.message)}
                </span>
              </>
            ) : null)}
          </div>
        </div>

        <div className="global-toggles">
          <LanguageToggle />
          <ThemeToggle />
        </div>
      </header>

      {gate?.ok && orderedNodes.length > 0 ? (
        <>
          <div className="env-section home-tray">
            <div className="env-head">
              <div className="t">
                <span className="home-block-title">{t("home.envTitle")}</span>
                <span className="s">{t("home.envSub")}</span>
              </div>
              <div className="env-head-actions">
                <button
                  type="button"
                  className="btn btn-primary action-btn"
                  disabled={envRunning || running}
                  onClick={() => void runEnvCheck()}
                >
                  {envRunning
                    ? t("home.envChecking")
                    : envOpen
                      ? t("home.envRecheck")
                      : t("home.envStart")}
                </button>
                {!envRunning && envCheckOk ? (
                  <span className="fetch-ok" role="status">
                    <span className="status-ok-mark">✓</span>
                    {t("home.success")}
                  </span>
                ) : !envRunning && cachedEnvTimeText ? (
                  <span className="cache-tag" role="status">
                    {t("home.cachedAt", { time: cachedEnvTimeText })}
                  </span>
                ) : null}
              </div>
            </div>
            {envOpen ? (
              <>
                {mixedPortNum == null || mixedPortNum <= 0 ? (
                  <div className="note note-compact env-tray-note">
                    {t("home.noProxyNote")}
                  </div>
                ) : null}
                <div className="card-grid card-grid-home env-tray-grid">
                  {envCards.map((c) => (
                    <CheckCardView key={c.id} card={c} />
                  ))}
                </div>
              </>
            ) : null}
          </div>

          <div className="node-section home-tray">
            <div className="ws-ops">
              <span className="t home-block-title">{t("home.nodeTitle")}</span>
              <div className="ws-ops-right">
                <div className="action-with-ok">
                  <button
                    type="button"
                    className="btn btn-primary action-btn"
                    disabled={running || clientUnset || filteredNodes.length === 0}
                    onClick={openConfirmAll}
                  >
                    {running && mode === "all"
                      ? isFiltered
                        ? t("home.testingFiltered")
                        : t("home.testingAll")
                      : isFiltered
                        ? t("home.testFiltered")
                        : t("home.testAll")}
                  </button>
                  {!running && nodeTestOk ? (
                    <span className="fetch-ok" role="status">
                      <span className="status-ok-mark">✓</span>
                      {t("home.success")}
                    </span>
                  ) : !running && cachedNodesTimeText ? (
                    <span className="cache-tag" role="status">
                      {t("home.cachedAt", { time: cachedNodesTimeText })}
                    </span>
                  ) : null}
                  {restoreError ? (
                    <span className="ws-restore-error" role="alert">
                      {td(restoreError)}
                    </span>
                  ) : null}
                </div>
                {running && mode === "all" ? (
                  <div
                    className="ws-progress-group"
                    role="status"
                    aria-live="polite"
                  >
                    <span className="ws-progress-main">
                      <span className="ws-progress-text">
                        {progressDisplay ? (
                          <>
                            <span className="ws-progress-phase">
                              {td(progressDisplay.phase)}
                            </span>
                            {progressDisplay.node ? (
                              <span className="ws-progress-node">
                                （
                                <span
                                  className="ws-progress-node-name"
                                  title={progressDisplay.node}
                                >
                                  {td(progressDisplay.node)}
                                </span>
                                ）
                              </span>
                            ) : null}
                          </>
                        ) : null}
                      </span>
                      <span
                        className="ws-progress-bar"
                        role="progressbar"
                        aria-valuemin={0}
                        aria-valuemax={100}
                        aria-valuenow={barPct}
                      >
                        <span
                          className="ws-progress-fill"
                          style={{ width: `${barPct}%` }}
                        />
                      </span>
                    </span>
                    <button
                      type="button"
                      className="btn btn-sm ws-abort-btn"
                      onClick={onAbortAll}
                    >
                      {t("home.stopRestore")}
                    </button>
                  </div>
                ) : running && testingNode ? (
                  <div className="ws-progress" role="status" aria-live="polite">
                    <span className="ws-progress-text">
                      <span className="ws-progress-phase">
                        {progressDisplay
                          ? td(progressDisplay.phase)
                          : t("home.testingNode")}
                      </span>
                      {(progressDisplay ? progressDisplay.node : testingNode) ? (
                        <span className="ws-progress-node">
                          （
                          <span
                            className="ws-progress-node-name"
                            title={
                              (progressDisplay
                                ? progressDisplay.node
                                : testingNode) ?? undefined
                            }
                          >
                            {td(
                              (progressDisplay
                                ? progressDisplay.node
                                : testingNode) ?? "",
                            )}
                          </span>
                          ）
                        </span>
                      ) : null}
                    </span>
                  </div>
                ) : null}
                {switchHint && !restoreError ? (
                  <span className="ws-error" role="alert">
                    {td(switchHint)}
                  </span>
                ) : null}
                <button
                  type="button"
                  className="ws-expand-link"
                  disabled={filteredNodes.length === 0}
                  onClick={toggleExpandAll}
                  title={
                    allExpanded
                      ? t("home.collapseAllTitle")
                      : t("home.expandAllTitle")
                  }
                >
                  {allExpanded ? t("home.collapseAll") : t("home.expandAll")}
                </button>
              </div>
            </div>

            <div
              className="node-filter-bar"
              role="region"
              aria-label={t("home.filterRegionAria")}
            >
              <div className="nf-pills">
                <button
                  type="button"
                  className={`nf-pill${regionFilter === null ? " active" : ""}`}
                  onClick={() => setRegionFilter(null)}
                >
                  <span>{t("home.filterAll")}</span>
                  <span className="nf-count">{orderedNodes.length}</span>
                </button>
                {inlineRegions.map(([reg, list]) => {
                  const active = regionFilter === reg;
                  return (
                    <button
                      key={reg}
                      type="button"
                      className={`nf-pill${active ? " active" : ""}`}
                      onClick={() => setRegionFilter(active ? null : reg)}
                    >
                      <span>{td(shortRegionName(reg))}</span>
                      <span className="nf-count">{list.length}</span>
                    </button>
                  );
                })}
                {overflowRegions.length > 0 ? (
                  <select
                    className={`nf-more-select${overflowActive ? " active" : ""}`}
                    aria-label={t("home.filterRegionAria")}
                    value={overflowActive ? (regionFilter ?? "") : ""}
                    onChange={(e) => setRegionFilter(e.target.value || null)}
                  >
                    <option value="">
                      {t("home.filterMoreRegions", { n: overflowNodeCount })}
                    </option>
                    {overflowRegions.map(([reg, list]) => (
                      <option key={reg} value={reg}>
                        {td(shortRegionName(reg))} ({list.length})
                      </option>
                    ))}
                  </select>
                ) : null}
              </div>

              <select
                className={`nf-region-select${regionFilter !== null ? " active" : ""}`}
                aria-label={t("home.filterRegionAria")}
                value={regionFilter ?? ""}
                onChange={(e) => setRegionFilter(e.target.value || null)}
              >
                <option value="">
                  {t("home.filterAllRegions", { n: orderedNodes.length })}
                </option>
                {regionGroups.map(([reg, list]) => (
                  <option key={reg} value={reg}>
                    {td(shortRegionName(reg))} ({list.length})
                  </option>
                ))}
              </select>

              <div className="nf-search-wrap">
                <svg
                  className="nf-search-icon"
                  viewBox="0 0 16 16"
                  fill="none"
                  aria-hidden="true"
                >
                  <circle cx="7" cy="7" r="4.8" stroke="currentColor" strokeWidth="1.5" />
                  <path
                    d="M10.6 10.6L14 14"
                    stroke="currentColor"
                    strokeWidth="1.5"
                    strokeLinecap="round"
                  />
                </svg>
                <input
                  type="text"
                  className="nf-search-input"
                  value={searchQuery}
                  onChange={(e) => setSearchQuery(e.target.value)}
                  placeholder={t("home.filterSearchPlaceholder")}
                  aria-label={t("home.filterSearchPlaceholder")}
                />
                {searchQuery ? (
                  <button
                    type="button"
                    className="nf-search-clear"
                    aria-label={t("home.filterClearSearch")}
                    title={t("home.filterClearSearch")}
                    onClick={() => setSearchQuery("")}
                  >
                    ×
                  </button>
                ) : null}
              </div>
            </div>

            {filteredNodes.length > 0 ? (
              <div className="node-flow">
                {nodeColumns.map((col, ci) => (
                  <div className="flow-col" key={ci}>
                    {col.map((n) => (
                      <NodeCard
                        key={n.name}
                        node={n}
                        score={scoreByName.get(n.name)}
                        liveCards={testingNode === n.name ? nodeCards : undefined}
                        isCurrent={n.name === connection.currentProxy}
                        expanded={expanded.has(n.name)}
                        testing={testingNode === n.name}
                        retestLocked={running}
                        onToggle={() => toggleExpand(n.name)}
                        onTest={() => void testOneNode(n, expandNode)}
                      />
                    ))}
                  </div>
                ))}
              </div>
            ) : (
              <div className="node-empty" role="status">
                <span>{t("home.filterEmpty", { total: orderedNodes.length })}</span>
                <button
                  type="button"
                  className="btn btn-sm"
                  onClick={resetFilter}
                >
                  {t("home.filterReset")}
                </button>
              </div>
            )}
          </div>
        </>
      ) : null}

      {allConfirmOpen && mode === "all" ? (
        <div className="confirm-overlay">
          <div
            className="confirm-panel card"
            role="dialog"
            aria-modal="true"
            aria-label={t("home.confirmAllAria")}
          >
            <p className="confirm-body">
              {isFiltered
                ? t("home.confirmFilteredBody", { n: filteredNodes.length })
                : t("home.confirmAllBody")}
            </p>
            <div className="toolbar" style={{ marginBottom: 0 }}>
              <button
                className="btn btn-primary"
                type="button"
                disabled={clientUnset || filteredNodes.length === 0}
                onClick={() => confirmAll(filteredNodes, isFiltered)}
              >
                {t("home.ok")}
              </button>
              <button
                className="btn"
                type="button"
                onClick={() => setAllConfirmOpen(false)}
              >
                {t("home.cancel")}
              </button>
            </div>
          </div>
        </div>
      ) : null}

      <details className="about-footer">
        <summary>{t("home.about")}</summary>
        <div className="a-body">
          {t("home.repo")}
          <a
            href="https://github.com/JTee77/clash-assist"
            target="_blank"
            rel="noreferrer"
          >
            github.com/JTee77/clash-assist
          </a>
          <br />
          {t("home.thanks")}
          <br />
          <span className="donate-line">
            <span>{t("home.coffee")}</span>
            <code className="donate-addr">
              0xa1ee52b64c39B94B74E50cEBF5E667a2e57E57Cb
            </code>
            <button
              type="button"
              className="donate-copy-btn"
              onClick={() => {
                void navigator.clipboard
                  ?.writeText("0xa1ee52b64c39B94B74E50cEBF5E667a2e57E57Cb")
                  .then(() => {
                    setCopiedDonate(true);
                    setTimeout(() => setCopiedDonate(false), 1800);
                  })
                  .catch(() => {});
              }}
            >
              {copiedDonate ? t("home.copiedAddr") : t("home.copyAddr")}
            </button>
          </span>
        </div>
      </details>
    </div>
  );
}
