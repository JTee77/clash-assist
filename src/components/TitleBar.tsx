import { useI18n } from "../i18n";
import { useUpdateFlow } from "../hooks/useUpdateFlow";

type UpdateFlow = ReturnType<typeof useUpdateFlow>;

/**
 * 更新胶囊（Mac 标题栏与 Windows 更新条共用），按真实阶段清晰提示用户：
 * 1) 初始：`发现新版本 X.Y.Z  [一键更新]  ×`（绝不擅自后台联网下载，必须由用户主动点击）
 * 2) 下载中：`正在下载更新 X.Y.Z…`
 * 3) 安装中（仅 Mac）：`下载完成，正在后台静默安装…`（静默挂载 DMG、清除 quarantine、校验签名并替换 .app）
 * 4) 就绪（零打扰）：`新版本 X.Y.Z 已安装好  [立即重启]  ×`（由用户决定何时重启；若暂不重启，下次重开软件自动生效）
 * 5) 上次更新未完成：`上次更新未完成  [重试]  [前往发布页]  ×`
 */
export function UpdatePill({ flow }: { flow: UpdateFlow }) {
  const { t } = useI18n();
  const { view, state } = flow;
  if (!view.visible || !view.status) return null;
  const statusText = `${t(view.status.key, view.status.vars)}${
    view.percent != null ? ` ${view.percent}%` : ""
  }`;
  const title = view.hint ? t(view.hint) : (state.error ?? undefined);
  return (
    <span className="update-pill" role="status" title={title}>
      <i className="update-pill-dot" aria-hidden="true" />
      <span className="update-pill-text">{statusText}</span>
      {view.primary && (
        <button
          type="button"
          className="update-pill-go"
          onClick={() => void flow.primary()}
        >
          {t(view.primary)}
        </button>
      )}
      {view.showReleaseLink && (
        <button
          type="button"
          className="update-pill-go update-pill-go--secondary"
          onClick={flow.openRelease}
        >
          {t("update.openRelease")}
        </button>
      )}
      {view.showClose && (
        <button
          type="button"
          className="update-pill-close"
          aria-label={t("close")}
          title={t("close")}
          onClick={flow.dismiss}
        >
          ×
        </button>
      )}
    </span>
  );
}

/**
 * 自绘标题栏（仅 Mac，titleBarStyle=Overlay）：红绿灯右侧显示「Clash伴侣 版本」，
 * 有新版本时紧随一枚更新胶囊。Windows 用原生标题栏 + UpdateBar。
 */
export function TitleBar() {
  const flow = useUpdateFlow({ windows: false });
  const { t } = useI18n();

  return (
    <div className="titlebar" data-tauri-drag-region>
      <span className="titlebar-title" data-tauri-drag-region>
        {t("appName")} {__APP_VERSION__}
      </span>
      <UpdatePill flow={flow} />
    </div>
  );
}
