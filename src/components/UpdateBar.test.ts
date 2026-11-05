import { beforeEach, describe, expect, it, vi } from "vitest";

const h = vi.hoisted(() => ({ invoke: vi.fn(), listen: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: h.invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: h.listen }));

import { UpdateBar } from "./UpdateBar";
import { updaterSupported, WINDOWS_UPDATER_ENABLED } from "../lib/update";

describe("UpdateBar（M5 起 Windows 更新器开启）", () => {
  beforeEach(() => {
    h.invoke.mockReset();
    h.listen.mockReset();
  });

  it("开关已打开：渲染 Windows 更新条组件（由它挂载 useUpdateFlow 做启动检查）", () => {
    expect(WINDOWS_UPDATER_ENABLED).toBe(true);
    expect(updaterSupported(true)).toBe(true);
    const el = UpdateBar();
    expect(el).not.toBeNull();
    // 只构造元素、不渲染：此时还不应调用任何命令
    expect(h.invoke).not.toHaveBeenCalled();
    expect(h.listen).not.toHaveBeenCalled();
  });
});
