import { beforeEach, describe, expect, it, vi } from "vitest";

const h = vi.hoisted(() => ({
  invoke: vi.fn(),
}));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: h.invoke,
}));

import {
  __resetUpdateCheckForTests,
  applyUpdate,
  canOneClick,
  checkUpdateOnce,
  downloadUpdate,
  openReleasePage,
  openVergeReleasePage,
  relaunchUpdatedApp,
} from "./index";

describe("in-app staged self-update bridge (0.2.4)", () => {
  beforeEach(() => {
    h.invoke.mockReset();
    __resetUpdateCheckForTests();
    (globalThis as unknown as { window?: Record<string, unknown> }).window = {
      __TAURI_INTERNALS__: {},
    };
  });

  it("checkUpdateOnce caches the promise across calls and preserves dmg_url / dmg_sha256", async () => {
    h.invoke.mockResolvedValue({
      current: "0.2.4",
      latest: "0.2.5",
      update_available: true,
      url: "https://github.com/JTee77/clash-assist/releases/tag/v0.2.5",
      dmg_url:
        "https://github.com/JTee77/clash-assist/releases/download/v0.2.5/Clash.Assist_0.2.5_aarch64.dmg",
      dmg_sha256:
        "sha256:251112b7cd87706978a3ad57b15b3d4191dfacc65fefa8d9b5c36da01482dbdc",
    });

    const first = await checkUpdateOnce();
    const second = await checkUpdateOnce();
    expect(h.invoke).toHaveBeenCalledTimes(1);
    expect(h.invoke).toHaveBeenCalledWith("check_update");
    expect(first).toEqual(second);
    expect(first?.dmg_url).toContain("Clash.Assist_0.2.5_aarch64.dmg");
  });

  it("runs staged downloadUpdate -> applyUpdate -> relaunchUpdatedApp commands in order", async () => {
    h.invoke
      .mockResolvedValueOnce("/tmp/clash-assist-update-123")
      .mockResolvedValueOnce("/Applications/Clash Assist.app")
      .mockResolvedValueOnce(undefined);

    const workDir = await downloadUpdate();
    expect(workDir).toBe("/tmp/clash-assist-update-123");
    // 0.2.9：前端不再传下载地址 / 摘要，Rust 侧自行解析（防止被注入的前端指定任意安装包）
    expect(h.invoke).toHaveBeenNthCalledWith(1, "download_update");

    const targetApp = await applyUpdate(workDir);
    expect(targetApp).toBe("/Applications/Clash Assist.app");
    expect(h.invoke).toHaveBeenNthCalledWith(2, "apply_update", {
      workDir: "/tmp/clash-assist-update-123",
    });

    await relaunchUpdatedApp();
    expect(h.invoke).toHaveBeenNthCalledWith(3, "relaunch_updated_app");
  });

  it("canOneClick requires both dmg_url and dmg_sha256", () => {
    const base = {
      current: "0.2.8",
      latest: "0.2.9",
      update_available: true,
      url: "https://github.com/JTee77/clash-assist/releases/tag/v0.2.9",
    };
    const dmg =
      "https://github.com/JTee77/clash-assist/releases/download/v0.2.9/Clash.Assist_0.2.9_aarch64.dmg";
    expect(canOneClick({ ...base, dmg_url: dmg, dmg_sha256: "sha256:abcd" })).toBe(true);
    expect(canOneClick({ ...base, dmg_url: dmg })).toBe(false);
    expect(canOneClick({ ...base, dmg_sha256: "sha256:abcd" })).toBe(false);
    expect(canOneClick(base)).toBe(false);
  });

  it("canOneClick: Windows 无 one_click 时走发布页；one_click / portable 由 Rust 决定（M5）", () => {
    const base = {
      current: "0.2.8",
      latest: "0.2.9",
      update_available: true,
      url: "https://github.com/JTee77/clash-assist/releases/tag/v0.2.9",
      dmg_url:
        "https://github.com/JTee77/clash-assist/releases/download/v0.2.9/Clash.Assist_0.2.9_aarch64.dmg",
      dmg_sha256: "sha256:abcd",
    };
    // Windows：Mac 的 DMG 字段不能触发一键更新
    expect(canOneClick(base, true)).toBe(false);
    expect(canOneClick(base, false)).toBe(true);
    // Rust 显式给出 one_click 时以它为准
    expect(canOneClick({ ...base, one_click: true, install_kind: "nsis" }, true)).toBe(true);
    expect(canOneClick({ ...base, one_click: false }, false)).toBe(false);
    // portable 永远只去发布页
    expect(canOneClick({ ...base, one_click: true, install_kind: "portable" }, true)).toBe(false);
  });

  it("openVergeReleasePage 只调用无参数命令，不传任何 URL", async () => {
    h.invoke.mockResolvedValue(undefined);
    await openVergeReleasePage();
    expect(h.invoke).toHaveBeenCalledTimes(1);
    expect(h.invoke.mock.calls[0]).toEqual(["open_verge_release_page"]);
    h.invoke.mockRejectedValue(new Error("blocked"));
    await expect(openVergeReleasePage()).resolves.toBeUndefined();
  });

  it("前端源码里不出现 Verge 发布页地址（只能由 Rust 常量决定）", () => {
    const sources = import.meta.glob(["../../**/*.{ts,tsx}", "!../../**/*.test.ts"], {
      query: "?raw",
      import: "default",
      eager: true,
    }) as Record<string, string>;
    expect(Object.keys(sources).length).toBeGreaterThan(10);
    for (const [file, src] of Object.entries(sources)) {
      expect(src.includes("clash-verge-rev/releases"), file).toBe(false);
    }
  });

  it("openReleasePage swallows errors gracefully", async () => {
    h.invoke.mockRejectedValue(new Error("blocked"));
    await expect(
      openReleasePage("https://github.com/JTee77/clash-assist/releases"),
    ).resolves.toBeUndefined();
  });
});
