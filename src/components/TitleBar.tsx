import { useI18n } from "../i18n";
import { useEffect, useState } from "react";
import {
  checkUpdateOnce,
  installUpdate,
  openReleasePage,
  type UpdateInfo,
} from "../lib/update";

/**
 * 自绘标题栏（titleBarStyle=Overlay）：红绿灯右侧显示「Clash伴侣 版本」，
 * 有新版本时紧随一枚胶囊：● 发现新版本 X.Y.Z  一键更新  ×。
 * 点击「一键更新」直接在应用内下载最新 DMG、清除 com.apple.quarantine 隔离标签、
 * 原位替换 .app 并自动重启，免去后续升级时的 Gatekeeper 弹窗与手动拖拽。
 * 若自更新遇到环境限制（如无写入权限或网络中断），自动回退打开 GitHub 下载页。
 */
export function TitleBar() {
  const [update, setUpdate] = useState<UpdateInfo | null>(null);
  const [dismissed, setDismissed] = useState(false);
  const [updating, setUpdating] = useState(false);
  const [fallbackError, setFallbackError] = useState<string | null>(null);
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

  const handleUpdateClick = async () => {
    if (!update || updating) return;
    if (!update.dmg_url || fallbackError) {
      void openReleasePage(update.url);
      return;
    }
    setUpdating(true);
    setFallbackError(null);
    try {
      await installUpdate(update);
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err ?? "");
      setUpdating(false);
      setFallbackError(msg || "update failed");
      void openReleasePage(update.url);
    }
  };

  return (
    <div className="titlebar" data-tauri-drag-region>
      <span className="titlebar-title" data-tauri-drag-region>
        {t("appName")} {__APP_VERSION__}
      </span>
      {update && !dismissed && (
        <span
          className="update-pill"
          role="status"
          title={fallbackError ?? undefined}
        >
          <i className="update-pill-dot" aria-hidden="true" />
          <span className="update-pill-text">
            {updating
              ? t("update.updating", { v: update.latest })
              : fallbackError
                ? t("update.fallbackOpened")
                : t("update.found", { v: update.latest })}
          </span>
          {!updating && (
            <button
              type="button"
              className="update-pill-go"
              onClick={() => void handleUpdateClick()}
            >
              {update.dmg_url && !fallbackError
                ? t("update.install")
                : t("update.go")}
            </button>
          )}
          {!updating && (
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
