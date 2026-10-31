import { listen } from "@tauri-apps/api/event";
import { useEffect, useReducer } from "react";
import type { MessageKey } from "../i18n";
import {
  applyUpdate,
  canOneClick,
  checkUpdateOnce,
  downloadUpdate,
  openReleasePage,
  relaunchUpdatedApp,
  updaterSupported,
  type UpdateInfo,
} from "../lib/update";

/** Rust stream_dmg_to_file 广播的下载进度（update_check.rs::DownloadProgress）。 */
type DownloadProgressPayload = {
  downloaded: number;
  total: number | null;
  percent: number | null;
};

/**
 * 更新状态机（Mac 标题栏 TitleBar 与 Windows 更新条 UpdateBar 共用，windows-plan §2.7）：
 * - Mac：idle → downloading → installing（静默挂载 DMG 替换 .app）→ ready → restarting
 * - Windows：idle → downloading → ready → restarting（安装器在重启时执行，§2.4-8）
 * - 任一步失败 → fallback（已为用户打开发布页）
 * - pending：上次更新未完成（Rust 启动对账给出），提供「重试」与「前往发布页」
 */
export type UpdateStage =
  | "idle"
  | "pending"
  | "downloading"
  | "installing"
  | "ready"
  | "restarting"
  | "fallback";

export type UpdateFlowState = {
  update: UpdateInfo | null;
  stage: UpdateStage;
  dismissed: boolean;
  error: string | null;
  downloadPct: number | null;
};

export type UpdateFlowEvent =
  | { type: "checked"; info: UpdateInfo | null }
  | { type: "download-start" }
  | { type: "progress"; percent: number }
  /** 下载并校验完成；needsInstall=true（Mac）进入 installing，否则（Windows）直接 ready。 */
  | { type: "downloaded"; needsInstall: boolean }
  | { type: "installed" }
  | { type: "restart-start" }
  | { type: "failed"; error: string }
  | { type: "dismiss" };

export const initialUpdateFlowState: UpdateFlowState = {
  update: null,
  stage: "idle",
  dismissed: false,
  error: null,
  downloadPct: null,
};

const BUSY: ReadonlySet<UpdateStage> = new Set(["downloading", "installing", "restarting"]);

export function isBusyStage(stage: UpdateStage): boolean {
  return BUSY.has(stage);
}

/** 纯状态迁移；非法事件（如忙碌中再次开始下载）原样返回当前状态。 */
export function updateFlowReducer(
  state: UpdateFlowState,
  event: UpdateFlowEvent,
): UpdateFlowState {
  switch (event.type) {
    case "checked": {
      if (state.update) return state;
      const info = event.info;
      if (!info) return state;
      if (info.pending) return { ...state, update: info, stage: "pending" };
      if (info.update_available) return { ...state, update: info, stage: "idle" };
      return state;
    }
    case "download-start":
      if (!state.update) return state;
      if (state.stage !== "idle" && state.stage !== "pending") return state;
      return { ...state, stage: "downloading", error: null, downloadPct: null };
    case "progress":
      if (state.stage !== "downloading") return state;
      return { ...state, downloadPct: event.percent };
    case "downloaded":
      if (state.stage !== "downloading") return state;
      return {
        ...state,
        stage: event.needsInstall ? "installing" : "ready",
        downloadPct: null,
      };
    case "installed":
      if (state.stage !== "installing") return state;
      return { ...state, stage: "ready" };
    case "restart-start":
      if (state.stage !== "ready") return state;
      return { ...state, stage: "restarting" };
    case "failed":
      if (!state.update) return state;
      return { ...state, stage: "fallback", error: event.error, downloadPct: null };
    case "dismiss":
      if (isBusyStage(state.stage)) return state;
      return { ...state, dismissed: true };
  }
}

/** 主按钮点击时应做的事（纯函数，便于测试）。 */
export type PrimaryAction = "none" | "download" | "relaunch" | "open-release";

export function primaryActionOf(state: UpdateFlowState, oneClick: boolean): PrimaryAction {
  if (!state.update || isBusyStage(state.stage)) return "none";
  if (state.stage === "ready") return "relaunch";
  if (state.stage === "fallback") return "open-release";
  return oneClick ? "download" : "open-release";
}

/** 更新条 / 胶囊要显示的文案 key（由组件用 t() 渲染）。 */
export type UpdateView = {
  visible: boolean;
  status: { key: MessageKey; vars?: Record<string, string> } | null;
  /** 附加在状态文案后的进度（仅 downloading）。 */
  percent: number | null;
  primary: MessageKey | null;
  /** pending 时额外的「前往发布页」按钮。 */
  showReleaseLink: boolean;
  showClose: boolean;
  /** 悬停提示的 i18n key；fallback 时组件改用 state.error 原文。 */
  hint: MessageKey | null;
};

export function updateViewOf(
  state: UpdateFlowState,
  opts: { oneClick: boolean; windows: boolean },
): UpdateView {
  const { update, stage } = state;
  const hidden: UpdateView = {
    visible: false,
    status: null,
    percent: null,
    primary: null,
    showReleaseLink: false,
    showClose: false,
    hint: null,
  };
  if (!update || state.dismissed) return hidden;
  const v = { v: update.latest };
  const busy = isBusyStage(stage);
  const portable = update.install_kind === "portable";
  const goKey: MessageKey = portable ? "update.goPortable" : "update.go";
  let status: UpdateView["status"];
  let primary: MessageKey | null = null;
  let hint: MessageKey | null = null;
  switch (stage) {
    case "pending":
      status = { key: "update.pending" };
      primary = opts.oneClick ? "update.retry" : goKey;
      break;
    case "downloading":
      status = { key: "update.downloading", vars: v };
      break;
    case "installing":
      status = { key: "update.installing" };
      break;
    case "ready":
      status = { key: opts.windows ? "update.readyWin" : "update.ready", vars: v };
      primary = "update.relaunch";
      hint = opts.windows ? "update.readyHintWin" : "update.readyHint";
      break;
    case "restarting":
      status = { key: "update.restarting" };
      break;
    case "fallback":
      status = { key: "update.fallbackOpened" };
      primary = goKey;
      break;
    default:
      status = { key: "update.found", vars: v };
      primary = opts.oneClick ? "update.install" : goKey;
  }
  return {
    visible: true,
    status,
    percent: stage === "downloading" ? state.downloadPct : null,
    primary: busy ? null : primary,
    showReleaseLink: stage === "pending" && opts.oneClick,
    showClose: !busy,
    hint,
  };
}

function errMsg(err: unknown, fallback: string): string {
  const msg = err instanceof Error ? err.message : String(err ?? "");
  return msg || fallback;
}

/** TitleBar（Mac）/ UpdateBar（Windows）共用的更新流程编排。 */
export function useUpdateFlow({ windows }: { windows: boolean }) {
  const [state, dispatch] = useReducer(updateFlowReducer, initialUpdateFlowState);
  const { update, stage } = state;
  const supported = updaterSupported(windows);
  const oneClick = update ? canOneClick(update, windows) : false;

  // 更新器关闭（M5 之前的 Windows）：不做启动检查，状态停在 idle 且无 update → 不显示任何入口。
  useEffect(() => {
    if (!supported) return;
    let alive = true;
    void checkUpdateOnce(windows).then((r) => {
      if (alive) dispatch({ type: "checked", info: r });
    });
    return () => {
      alive = false;
    };
  }, [supported, windows]);

  // 下载阶段订阅进度事件；离开阶段即退订（reducer 在离开 downloading 时清零百分比）。
  useEffect(() => {
    if (stage !== "downloading") return;
    let alive = true;
    let unlisten: (() => void) | null = null;
    void listen<DownloadProgressPayload>("update-download-progress", (e) => {
      const p = e.payload?.percent;
      if (typeof p === "number") dispatch({ type: "progress", percent: p });
    }).then((u) => {
      if (alive) unlisten = u;
      else u();
    });
    return () => {
      alive = false;
      unlisten?.();
    };
  }, [stage]);

  const fail = (err: unknown, fallback: string) => {
    dispatch({ type: "failed", error: errMsg(err, fallback) });
    if (update) void openReleasePage(update.url);
  };

  const primary = async () => {
    if (!update || !supported) return;
    const action = primaryActionOf(state, oneClick);
    if (action === "open-release") {
      void openReleasePage(update.url);
      return;
    }
    if (action === "relaunch") {
      dispatch({ type: "restart-start" });
      try {
        await relaunchUpdatedApp();
      } catch (err) {
        fail(err, "relaunch failed");
      }
      return;
    }
    if (action !== "download") return;
    dispatch({ type: "download-start" });
    try {
      const workDir = await downloadUpdate();
      // Windows：安装器在重启时执行（M5），下载完即就绪；Mac：后台静默替换 .app。
      dispatch({ type: "downloaded", needsInstall: !windows });
      if (!windows) {
        await applyUpdate(workDir);
        dispatch({ type: "installed" });
      }
    } catch (err) {
      fail(err, "update failed");
    }
  };

  const openRelease = () => {
    if (update) void openReleasePage(update.url);
  };

  const view = updateViewOf(state, { oneClick, windows });

  return {
    state,
    view,
    primary,
    openRelease,
    dismiss: () => dispatch({ type: "dismiss" }),
  };
}
