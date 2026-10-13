import { useEffect, useState } from "react";
import { checkUpdateOnce, openReleasePage, type UpdateInfo } from "../lib/update";

/**
 * 自绘标题栏（titleBarStyle=Overlay）：红绿灯右侧显示「Clash伴侣 版本」，
 * 有新版本时紧随一枚胶囊：● 发现新版本 X.Y.Z  去下载  ×。
 * 标题栏占据文档流里的 28px，不覆盖正文。× 只在本次启动内隐藏。
 */
export function TitleBar() {
  const [update, setUpdate] = useState<UpdateInfo | null>(null);
  const [dismissed, setDismissed] = useState(false);

  useEffect(() => {
    let alive = true;
    void checkUpdateOnce().then((r) => {
      if (alive && r?.update_available) setUpdate(r);
    });
    return () => {
      alive = false;
    };
  }, []);

  return (
    <div className="titlebar" data-tauri-drag-region>
      <span className="titlebar-title" data-tauri-drag-region>
        Clash伴侣 {__APP_VERSION__}
      </span>
      {update && !dismissed && (
        <span className="update-pill" role="status">
          <i className="update-pill-dot" aria-hidden="true" />
          <span className="update-pill-text">发现新版本 {update.latest}</span>
          <button
            type="button"
            className="update-pill-go"
            onClick={() => void openReleasePage(update.url)}
          >
            去下载
          </button>
          <button
            type="button"
            className="update-pill-close"
            aria-label="关闭"
            title="关闭"
            onClick={() => setDismissed(true)}
          >
            ×
          </button>
        </span>
      )}
    </div>
  );
}
