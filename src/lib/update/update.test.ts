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

  it("openReleasePage swallows errors gracefully", async () => {
    h.invoke.mockRejectedValue(new Error("blocked"));
    await expect(
      openReleasePage("https://github.com/JTee77/clash-assist/releases"),
    ).resolves.toBeUndefined();
  });
});
