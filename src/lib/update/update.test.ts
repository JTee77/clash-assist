import { beforeEach, describe, expect, it, vi } from "vitest";

const h = vi.hoisted(() => ({
  invoke: vi.fn(),
}));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: h.invoke,
}));

import {
  __resetUpdateCheckForTests,
  checkUpdateOnce,
  installUpdate,
  openReleasePage,
} from "./index";

describe("in-app self-update bridge (0.2.4)", () => {
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

  it("installUpdate forwards dmgUrl and dmgSha256 to install_update command", async () => {
    h.invoke.mockResolvedValue(undefined);
    await installUpdate({
      current: "0.2.4",
      latest: "0.2.5",
      update_available: true,
      url: "https://github.com/JTee77/clash-assist/releases/tag/v0.2.5",
      dmg_url:
        "https://github.com/JTee77/clash-assist/releases/download/v0.2.5/Clash.Assist_0.2.5_aarch64.dmg",
      dmg_sha256: "sha256:abcd",
    });
    expect(h.invoke).toHaveBeenCalledWith("install_update", {
      dmgUrl:
        "https://github.com/JTee77/clash-assist/releases/download/v0.2.5/Clash.Assist_0.2.5_aarch64.dmg",
      dmgSha256: "sha256:abcd",
    });
  });

  it("openReleasePage swallows errors gracefully", async () => {
    h.invoke.mockRejectedValue(new Error("blocked"));
    await expect(
      openReleasePage("https://github.com/JTee77/clash-assist/releases"),
    ).resolves.toBeUndefined();
  });
});
