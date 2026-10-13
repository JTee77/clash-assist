/**
 * 启动时更新检查（GUI）。HTTP 在 Rust `check_update`（reqwest，约 5 s 超时）；
 * 这里只保证「每次启动最多一次」并把任何失败静默吞掉（返回 null）。
 */
import { invoke } from "@tauri-apps/api/core";

export type UpdateInfo = {
  current: string;
  latest: string;
  update_available: boolean;
  url: string;
  fake?: boolean;
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

/** 测试用：重置「本次启动已检查」状态。 */
export function __resetUpdateCheckForTests(): void {
  once = null;
}
