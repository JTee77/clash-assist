import { afterEach, describe, expect, it, vi } from "vitest";
import { applyPlatformAttr, isMac, isWindows, platformOf } from "./platform";

const WEBVIEW2_UA =
  "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36 Edg/130.0.0.0";
const WKWEBVIEW_UA =
  "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko)";
afterEach(() => {
  vi.unstubAllGlobals();
});

const LINUX_UA = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36";

describe("platformOf / isWindows", () => {
  it("WebView2（Windows 10/11 x64 与 ARM64）识别为 windows", () => {
    expect(platformOf(WEBVIEW2_UA)).toBe("windows");
    expect(platformOf("Mozilla/5.0 (Windows NT 10.0; ARM64) Edg/130.0")).toBe("windows");
    expect(isWindows(WEBVIEW2_UA)).toBe(true);
    expect(isMac(WEBVIEW2_UA)).toBe(false);
  });

  it("WKWebView 识别为 macos（Apple Silicon 同样报 Intel Mac OS X）", () => {
    expect(platformOf(WKWEBVIEW_UA)).toBe("macos");
    expect(isMac(WKWEBVIEW_UA)).toBe(true);
    expect(isWindows(WKWEBVIEW_UA)).toBe(false);
  });

  it("其他 / 空 UA 归为 other，不误判为 Windows", () => {
    expect(platformOf(LINUX_UA)).toBe("other");
    expect(platformOf("")).toBe("other");
    expect(platformOf("Mozilla/5.0 (Darwin-ish; Window-manager)")).toBe("other");
  });

  it("默认参数读取 navigator.userAgent", () => {
    vi.stubGlobal("navigator", { userAgent: WEBVIEW2_UA });
    expect(isWindows()).toBe(true);
    vi.stubGlobal("navigator", { userAgent: WKWEBVIEW_UA });
    expect(isWindows()).toBe(false);
    expect(isMac()).toBe(true);
  });
});

describe("applyPlatformAttr", () => {
  it("写入 <html data-platform>", () => {
    const documentElement = { dataset: {} as Record<string, string> };
    vi.stubGlobal("document", { documentElement });
    expect(applyPlatformAttr("windows")).toBe("windows");
    expect(documentElement.dataset.platform).toBe("windows");
    applyPlatformAttr("macos");
    expect(documentElement.dataset.platform).toBe("macos");
  });

  it("无 DOM 时不抛错", () => {
    expect(applyPlatformAttr("other")).toBe("other");
  });
});
