import { beforeEach, describe, expect, it, vi } from "vitest";

const h = vi.hoisted(() => ({ invoke: vi.fn(), listen: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: h.invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: h.listen }));

import { UpdateBar } from "./UpdateBar";
import { WINDOWS_UPDATER_ENABLED } from "../lib/update";

describe("UpdateBar（M5 之前 Windows 更新器关闭）", () => {
  beforeEach(() => {
    h.invoke.mockReset();
    h.listen.mockReset();
  });

  it("不渲染更新入口，也不调用任何更新命令", () => {
    expect(WINDOWS_UPDATER_ENABLED).toBe(false);
    expect(UpdateBar()).toBeNull();
    expect(h.invoke).not.toHaveBeenCalled();
    expect(h.listen).not.toHaveBeenCalled();
  });
});
