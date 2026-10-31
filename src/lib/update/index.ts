/**
 * 启动时更新检查与应用内分阶段自更新（GUI）。
 * 1) `downloadUpdate`：用户点击「一键更新」后，下载最新 DMG 并校验 SHA-256。
 * 2) `applyUpdate`：下载完成后在后台静默挂载 DMG、清除 com.apple.quarantine 隔离标签、替换当前 .app（不强行退出）。
 * 3) `relaunchUpdatedApp`：由用户主动点击「立即重启」时拉起新版并退出（若不点，下次重开自动生效）。
 */
import { invoke } from "@tauri-apps/api/core";
import { isWindows } from "../platform";

/**
 * 安装形态（windows-plan §2.4 / M5，由 Rust 判定）：portable 只能去发布页手动下载。
 * 旧版 Rust（M5 之前）不返回该字段。
 */
export type InstallKind = "dmg" | "nsis" | "portable";

/**
 * 上次更新未完成（windows-plan §2.4-8，M5 由 Rust 启动对账后填入）：
 * 前端据此在更新条里提示「上次更新未完成」，提供「重试」与「前往发布页」。
 */
export type UpdatePending = {
  from: string;
  to: string;
};

export type UpdateInfo = {
  current: string;
  latest: string;
  update_available: boolean;
  url: string;
  fake?: boolean;
  dmg_url?: string;
  dmg_sha256?: string;
  /** M5：Rust 判定能否一键更新（有此字段时以它为准）。 */
  one_click?: boolean;
  install_kind?: InstallKind;
  pending?: UpdatePending | null;
};

/**
 * Windows 的应用内更新在 M5 之前关闭（与 Rust `update_check::WINDOWS_UPDATER_ENABLED` 一致）：
 * 不自动检查、不显示更新条；Rust 侧四个更新命令也会直接返回 updater_unsupported。
 */
export const WINDOWS_UPDATER_ENABLED = false;

export function updaterSupported(windows: boolean = isWindows()): boolean {
  return !windows || WINDOWS_UPDATER_ENABLED;
}

let once: Promise<UpdateInfo | null> | null = null;

function isTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

/**
 * 每次启动只发一次请求（React StrictMode 双调用 / 重挂载都复用同一个 Promise）。
 * 更新器关闭的平台（M5 之前的 Windows）不调用 check_update，直接视为「无更新」。
 */
export function checkUpdateOnce(windows: boolean = isWindows()): Promise<UpdateInfo | null> {
  if (!once) {
    once = isTauri() && updaterSupported(windows)
      ? invoke<UpdateInfo>("check_update").then(
          (r) => r,
          () => null,
        )
      : Promise.resolve(null);
  }
  return once;
}

export function openReleasePage(url: string): Promise<void> {
  return invoke<void>("open_release_page", { url }).catch(() => {});
}

/**
 * 打开 Clash Verge Rev 最新版发布页（InsecureDacl 提示的升级按钮）。
 * 无参数：地址是 Rust 端的常量 VERGE_LATEST_RELEASE_PAGE，前端不能指定 URL。
 * 失败时照常 reject，由调用方给用户反馈（PR52 Ellie 建议）。
 */
export function openVergeReleasePage(): Promise<void> {
  return invoke<void>("open_verge_release_page");
}

/**
 * 能否走「一键更新」：
 * - Rust 返回了 `one_click`（M5）→ 以它为准；portable 一律不行（只能去发布页下载）；
 * - Windows 上 Rust 尚未给出 `one_click` → 不走一键更新（Mac 的 DMG 安装链在 Windows 不可用），直接去发布页；
 * - Mac：release 同时提供 .dmg 直链与 SHA-256 摘要才行。缺摘要时 Rust 侧会拒绝下载（fail closed），
 *   这里提前改走「去下载页」，避免先报错再回落。
 */
export function canOneClick(info: UpdateInfo, windows: boolean = isWindows()): boolean {
  if (info.install_kind === "portable") return false;
  if (typeof info.one_click === "boolean") return info.one_click;
  if (windows) return false;
  return Boolean(info.dmg_url && info.dmg_sha256);
}

/**
 * 阶段 1：下载最新 DMG 到临时目录并完成 SHA-256 校验，返回临时工作目录路径。
 * 不传任何参数：下载地址与期望摘要由 Rust 侧自行从 GitHub release 元数据解析，前端无法指定。
 */
export function downloadUpdate(): Promise<string> {
  return invoke<string>("download_update");
}

/** 阶段 2：后台静默挂载 DMG、清除 quarantine 并替换当前 .app（不自动退出重启）。 */
export function applyUpdate(workDir: string): Promise<string> {
  return invoke<string>("apply_update", { workDir });
}

/** 阶段 3：用户主动点击「立即重启」时拉起已替换的新版 .app 并退出当前进程。 */
export function relaunchUpdatedApp(): Promise<void> {
  return invoke<void>("relaunch_updated_app");
}

/** 测试用：重置「本次启动已检查」状态。 */
export function __resetUpdateCheckForTests(): void {
  once = null;
}
