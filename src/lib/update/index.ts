/**
 * 启动时更新检查与应用内一键自更新（GUI）。
 * HTTP 与 DMG 挂载替换均在 Rust `update_check` 中完成；
 * 应用内自更新直接替换 .app 并清除 com.apple.quarantine，重启后不触发 Gatekeeper 弹窗。
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

/**
 * 应用内一键下载最新 DMG、静默提取替换当前 .app（清除 quarantine）并自动重启。
 * 若失败则抛出错误，由调用方回退打开浏览器下载页。
 */
export function installUpdate(info: UpdateInfo): Promise<void> {
  return invoke<void>("install_update", {
    dmgUrl: info.dmg_url ?? null,
    dmgSha256: info.dmg_sha256 ?? null,
  });
}

/** 测试用：重置「本次启动已检查」状态。 */
export function __resetUpdateCheckForTests(): void {
  once = null;
}
