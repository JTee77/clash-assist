/**
 * 启动时更新检查与应用内分阶段自更新（GUI）。
 * 1) `downloadUpdate`：用户点击「一键更新」后，下载最新 DMG 并校验 SHA-256。
 * 2) `applyUpdate`：下载完成后在后台静默挂载 DMG、清除 com.apple.quarantine 隔离标签、替换当前 .app（不强行退出）。
 * 3) `relaunchUpdatedApp`：由用户主动点击「立即重启」时拉起新版并退出（若不点，下次重开自动生效）。
 */
import { invoke } from "@tauri-apps/api/core";

export type UpdateInfo = {
  current: string;
  latest: string;
  update_available: boolean;
  url: string;
  fake?: boolean;
  dmg_url?: string;
  dmg_sha256?: string;
};

let once: Promise<UpdateInfo | null> | null = null;

function isTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

/** 每次启动只发一次请求（React StrictMode 双调用 / 重挂载都复用同一个 Promise）。 */
export function checkUpdateOnce(): Promise<UpdateInfo | null> {
  if (!once) {
    once = isTauri()
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

/** 阶段 1：下载最新 DMG 到临时目录并完成 SHA-256 校验，返回临时工作目录路径。 */
export function downloadUpdate(info: UpdateInfo): Promise<string> {
  return invoke<string>("download_update", {
    dmgUrl: info.dmg_url ?? null,
    dmgSha256: info.dmg_sha256 ?? null,
  });
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
