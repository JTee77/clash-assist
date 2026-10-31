/**
 * 前端平台识别（windows-plan §2.7）：只看 navigator.userAgent。
 * WebView2 的 UA 形如 `Mozilla/5.0 (Windows NT 10.0; Win64; x64) … Edg/…`，
 * WKWebView 为 `Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) …`（Apple Silicon 也报 Intel）。
 * 结果写到 `<html data-platform>`，供 CSS 按平台区分（玻璃 / 自绘标题栏只在 Mac）。
 */
export type Platform = "windows" | "macos" | "other";

export function platformOf(ua: string): Platform {
  if (/\bWindows\b|\bWin(32|64)\b/i.test(ua)) return "windows";
  if (/\bMacintosh\b|\bMac OS X\b/i.test(ua)) return "macos";
  return "other";
}

function currentUa(): string {
  return typeof navigator !== "undefined" && typeof navigator.userAgent === "string"
    ? navigator.userAgent
    : "";
}

export function currentPlatform(): Platform {
  return platformOf(currentUa());
}

export function isWindows(ua: string = currentUa()): boolean {
  return platformOf(ua) === "windows";
}

export function isMac(ua: string = currentUa()): boolean {
  return platformOf(ua) === "macos";
}

/** 在 `<html>` 上写 `data-platform`（幂等）；无 DOM（单测 / SSR）时直接返回平台值。 */
export function applyPlatformAttr(platform: Platform = currentPlatform()): Platform {
  if (typeof document !== "undefined" && document.documentElement) {
    document.documentElement.dataset.platform = platform;
  }
  return platform;
}
