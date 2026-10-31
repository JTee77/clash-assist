import { describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

import { openVergeWithFeedback } from "./DiagnosticsNotices";

describe("openVergeWithFeedback（升级按钮失败要有反馈）", () => {
  it("成功：先清掉旧提示，不报失败", async () => {
    const setFailed = vi.fn();
    await openVergeWithFeedback(setFailed, () => Promise.resolve());
    expect(setFailed.mock.calls).toEqual([[false]]);
  });

  it("失败：不吞错误，标记失败让按钮旁出一句提示", async () => {
    const setFailed = vi.fn();
    await openVergeWithFeedback(setFailed, () => Promise.reject(new Error("打开浏览器失败")));
    expect(setFailed.mock.calls).toEqual([[false], [true]]);
  });
});
