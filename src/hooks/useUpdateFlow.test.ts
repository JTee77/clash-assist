import { describe, expect, it } from "vitest";
import {
  initialUpdateFlowState,
  isBusyStage,
  primaryActionOf,
  updateFlowReducer,
  updateViewOf,
  type UpdateFlowEvent,
  type UpdateFlowState,
} from "./useUpdateFlow";
import type { UpdateInfo } from "../lib/update";

const INFO: UpdateInfo = {
  current: "0.2.9",
  latest: "0.3.0",
  update_available: true,
  url: "https://github.com/JTee77/clash-assist/releases/tag/v0.3.0",
};

function run(events: UpdateFlowEvent[], from: UpdateFlowState = initialUpdateFlowState) {
  return events.reduce(updateFlowReducer, from);
}

describe("updateFlowReducer 主流程", () => {
  it("没有新版本 / 检查失败时保持隐藏", () => {
    expect(run([{ type: "checked", info: null }])).toEqual(initialUpdateFlowState);
    const noUpdate = run([{ type: "checked", info: { ...INFO, update_available: false } }]);
    expect(noUpdate.update).toBeNull();
    expect(updateViewOf(noUpdate, { oneClick: true, windows: true }).visible).toBe(false);
  });

  it("Mac：idle → downloading → installing → ready → restarting", () => {
    let s = run([{ type: "checked", info: INFO }]);
    expect(s.stage).toBe("idle");
    s = run([{ type: "download-start" }, { type: "progress", percent: 42 }], s);
    expect(s).toMatchObject({ stage: "downloading", downloadPct: 42 });
    s = run([{ type: "downloaded", needsInstall: true }], s);
    expect(s).toMatchObject({ stage: "installing", downloadPct: null });
    s = run([{ type: "installed" }], s);
    expect(s.stage).toBe("ready");
    s = run([{ type: "restart-start" }], s);
    expect(s.stage).toBe("restarting");
  });

  it("Windows：downloading 之后直接 ready（不经过 installing）", () => {
    const s = run([
      { type: "checked", info: INFO },
      { type: "download-start" },
      { type: "downloaded", needsInstall: false },
    ]);
    expect(s.stage).toBe("ready");
    // installed 事件对 Windows 的 ready 无效
    expect(run([{ type: "installed" }], s).stage).toBe("ready");
  });

  it("忙碌阶段忽略重复开始、关闭与乱序事件", () => {
    const downloading = run([{ type: "checked", info: INFO }, { type: "download-start" }]);
    expect(isBusyStage(downloading.stage)).toBe(true);
    expect(run([{ type: "download-start" }], downloading)).toBe(downloading);
    expect(run([{ type: "dismiss" }], downloading).dismissed).toBe(false);
    expect(run([{ type: "restart-start" }], downloading)).toBe(downloading);
    expect(run([{ type: "installed" }], downloading)).toBe(downloading);
    // 只记录 downloading 阶段的进度
    const idle = run([{ type: "checked", info: INFO }]);
    expect(run([{ type: "progress", percent: 10 }], idle)).toBe(idle);
  });

  it("第二次 checked 不覆盖已有结果（StrictMode 双调用）", () => {
    const s = run([{ type: "checked", info: INFO }]);
    expect(run([{ type: "checked", info: { ...INFO, latest: "9.9.9" } }], s)).toBe(s);
  });
});

describe("fallback", () => {
  it("下载 / 安装 / 重启任一步失败都进入 fallback 并记录原因", () => {
    for (const before of [
      [{ type: "download-start" }],
      [{ type: "download-start" }, { type: "downloaded", needsInstall: true }],
      [
        { type: "download-start" },
        { type: "downloaded", needsInstall: false },
        { type: "restart-start" },
      ],
    ] as UpdateFlowEvent[][]) {
      const s = run([
        { type: "checked", info: INFO },
        ...before,
        { type: "failed", error: "sha256 mismatch" },
      ]);
      expect(s).toMatchObject({ stage: "fallback", error: "sha256 mismatch", downloadPct: null });
      expect(primaryActionOf(s, true)).toBe("open-release");
      const view = updateViewOf(s, { oneClick: true, windows: true });
      expect(view.status?.key).toBe("update.fallbackOpened");
      expect(view.primary).toBe("update.go");
      expect(view.showClose).toBe(true);
    }
  });

  it("fallback 后不能再开始下载", () => {
    const s = run([
      { type: "checked", info: INFO },
      { type: "download-start" },
      { type: "failed", error: "x" },
    ]);
    expect(run([{ type: "download-start" }], s)).toBe(s);
  });

  it("没有更新信息时 failed 不生效", () => {
    expect(run([{ type: "failed", error: "x" }])).toEqual(initialUpdateFlowState);
  });
});

describe("pending（上次更新未完成）", () => {
  const PENDING: UpdateInfo = { ...INFO, pending: { from: "0.2.9", to: "0.3.0" } };

  it("checked 带 pending → pending 阶段，即使没有 update_available 也显示", () => {
    const s = run([{ type: "checked", info: { ...PENDING, update_available: false } }]);
    expect(s.stage).toBe("pending");
    const view = updateViewOf(s, { oneClick: false, windows: true });
    expect(view.visible).toBe(true);
    expect(view.status?.key).toBe("update.pending");
  });

  it("可一键更新：「重试」走下载，另有「前往发布页」", () => {
    const s = run([{ type: "checked", info: PENDING }]);
    expect(primaryActionOf(s, true)).toBe("download");
    const view = updateViewOf(s, { oneClick: true, windows: true });
    expect(view.primary).toBe("update.retry");
    expect(view.showReleaseLink).toBe(true);
    expect(run([{ type: "download-start" }], s).stage).toBe("downloading");
  });

  it("不能一键更新：主按钮就是前往发布页，不再重复显示", () => {
    const s = run([{ type: "checked", info: PENDING }]);
    expect(primaryActionOf(s, false)).toBe("open-release");
    const view = updateViewOf(s, { oneClick: false, windows: true });
    expect(view.primary).toBe("update.go");
    expect(view.showReleaseLink).toBe(false);
  });

  it("重试失败 → fallback", () => {
    const s = run([
      { type: "checked", info: PENDING },
      { type: "download-start" },
      { type: "failed", error: "network" },
    ]);
    expect(s.stage).toBe("fallback");
  });
});

describe("updateViewOf / primaryActionOf", () => {
  it("初始：可一键更新显示「一键更新」，否则「去下载」；portable 显示「前往发布页下载」", () => {
    const s = run([{ type: "checked", info: INFO }]);
    expect(updateViewOf(s, { oneClick: true, windows: false }).primary).toBe("update.install");
    expect(primaryActionOf(s, true)).toBe("download");
    expect(updateViewOf(s, { oneClick: false, windows: false }).primary).toBe("update.go");
    expect(primaryActionOf(s, false)).toBe("open-release");
    const portable = run([{ type: "checked", info: { ...INFO, install_kind: "portable" } }]);
    expect(updateViewOf(portable, { oneClick: false, windows: true }).primary).toBe(
      "update.goPortable",
    );
  });

  it("下载中隐藏按钮并带上百分比", () => {
    const s = run([
      { type: "checked", info: INFO },
      { type: "download-start" },
      { type: "progress", percent: 7 },
    ]);
    const view = updateViewOf(s, { oneClick: true, windows: true });
    expect(view).toMatchObject({
      primary: null,
      showClose: false,
      percent: 7,
      status: { key: "update.downloading", vars: { v: "0.3.0" } },
    });
    expect(primaryActionOf(s, true)).toBe("none");
  });

  it("ready：Mac 显示「已安装好」，Windows 显示「已下载好」，主按钮均为立即重启", () => {
    const mac = run([
      { type: "checked", info: INFO },
      { type: "download-start" },
      { type: "downloaded", needsInstall: true },
      { type: "installed" },
    ]);
    expect(updateViewOf(mac, { oneClick: true, windows: false })).toMatchObject({
      status: { key: "update.ready" },
      primary: "update.relaunch",
      hint: "update.readyHint",
    });
    const win = run([
      { type: "checked", info: INFO },
      { type: "download-start" },
      { type: "downloaded", needsInstall: false },
    ]);
    expect(updateViewOf(win, { oneClick: true, windows: true })).toMatchObject({
      status: { key: "update.readyWin" },
      primary: "update.relaunch",
      hint: "update.readyHintWin",
    });
    expect(primaryActionOf(win, true)).toBe("relaunch");
  });

  it("关闭后隐藏", () => {
    const s = run([{ type: "checked", info: INFO }, { type: "dismiss" }]);
    expect(updateViewOf(s, { oneClick: true, windows: true }).visible).toBe(false);
  });
});
