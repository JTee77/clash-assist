import { useI18n } from "../i18n";
import { listen } from "@tauri-apps/api/event";
import { useEffect, useState } from "react";
import {
  applyUpdate,
  checkUpdateOnce,
  canOneClick,
  downloadUpdate,
  openReleasePage,
  relaunchUpdatedApp,
  type UpdateInfo,
} from "../lib/update";

/** Rust stream_dmg_to_file 广播的下载进度（update_check.rs::DownloadProgress）。 */
type DownloadProgressPayload = {
  downloaded: number;
  total: number | null;
  percent: number | null;
};

type UpdateStage =
  | "idle"
  | "downloading"
  | "installing"
  | "ready"
  | "restarting"
  | "fallback";

/**
 * 自绘标题栏（titleBarStyle=Overlay）：红绿灯右侧显示「Clash伴侣 版本」，
 * 有新版本时紧随一枚胶囊，按真实阶段清晰提示用户：
 * 1) 初始：`发现新版本 X.Y.Z  [一键更新]  ×`（绝不擅自后台联网下载，必须由用户主动点击）
 * 2) 下载中：`正在下载更新 X.Y.Z…`
 * 3) 安装中：`下载完成，正在后台静默安装…`（静默挂载 DMG、清除 quarantine、校验签名并替换 .app）
 * 4) 就绪（零打扰）：`新版本 X.Y.Z 已安装好  [立即重启]  ×`（由用户决定何时重启；若暂不重启，下次重开软件自动生效）
 */
export function TitleBar() {
  const [update, setUpdate] = useState<UpdateInfo | null>(null);
  const [dismissed, setDismissed] = useState(false);
  const [stage, setStage] = useState<UpdateStage>("idle");
  const [fallbackError, setFallbackError] = useState<string | null>(null);
  const [downloadPct, setDownloadPct] = useState<number | null>(null);
  const { t } = useI18n();

  useEffect(() => {
    let alive = true;
    void checkUpdateOnce().then((r) => {
      if (alive && r?.update_available) setUpdate(r);
    });
    return () => {
      alive = false;
    };
  }, []);

  // 下载阶段订阅进度事件；离开阶段即退订并清零（重新获取期间隐去上一轮状态）。
  useEffect(() => {
    if (stage !== "downloading") return;
    let alive = true;
    let unlisten: (() => void) | null = null;
    void listen<DownloadProgressPayload>("update-download-progress", (e) => {
      const p = e.payload?.percent;
      if (typeof p === "number") setDownloadPct(p);
    }).then((u) => {
      if (alive) unlisten = u;
      else u();
    });
    return () => {
      alive = false;
      unlisten?.();
      setDownloadPct(null);
    };
  }, [stage]);

  const handlePrimaryClick = async () => {
    if (!update) return;
    if (stage === "downloading" || stage === "installing" || stage === "restarting") {
      return;
    }
    if (stage === "ready") {
      setStage("restarting");
      try {
        await relaunchUpdatedApp();
      } catch (err) {
        const msg = err instanceof Error ? err.message : String(err ?? "");
        setStage("fallback");
        setFallbackError(msg || "relaunch failed");
        void openReleasePage(update.url);
      }
      return;
    }
    if (!canOneClick(update) || stage === "fallback") {
      void openReleasePage(update.url);
      return;
    }

    setStage("downloading");
    setFallbackError(null);
    try {
      const workDir = await downloadUpdate();
      setStage("installing");
      await applyUpdate(workDir);
      setStage("ready");
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err ?? "");
      setStage("fallback");
      setFallbackError(msg || "update failed");
      void openReleasePage(update.url);
    }
  };

  const busy =
    stage === "downloading" ||
    stage === "installing" ||
    stage === "restarting";

  const statusText = update
    ? stage === "downloading"
      ? `${t("update.downloading", { v: update.latest })}${
          downloadPct != null ? ` ${downloadPct}%` : ""
        }`
      : stage === "installing"
        ? t("update.installing")
        : stage === "ready"
          ? t("update.ready", { v: update.latest })
          : stage === "restarting"
            ? t("update.restarting")
            : stage === "fallback"
              ? t("update.fallbackOpened")
              : t("update.found", { v: update.latest })
    : "";

  const pillTitle =
    stage === "ready"
      ? t("update.readyHint")
      : fallbackError ?? undefined;

  return (
    <div className="titlebar" data-tauri-drag-region>
      <span className="titlebar-title" data-tauri-drag-region>
        {t("appName")} {__APP_VERSION__}
      </span>
      {update && !dismissed && (
        <span className="update-pill" role="status" title={pillTitle}>
          <i className="update-pill-dot" aria-hidden="true" />
          <span className="update-pill-text">{statusText}</span>
          {!busy && (
            <button
              type="button"
              className="update-pill-go"
              onClick={() => void handlePrimaryClick()}
            >
              {stage === "ready"
                ? t("update.relaunch")
                : canOneClick(update) && stage !== "fallback"
                  ? t("update.install")
                  : t("update.go")}
            </button>
          )}
          {!busy && (
            <button
              type="button"
              className="update-pill-close"
              aria-label={t("close")}
              title={t("close")}
              onClick={() => setDismissed(true)}
            >
              ×
            </button>
          )}
        </span>
      )}
    </div>
  );
}
