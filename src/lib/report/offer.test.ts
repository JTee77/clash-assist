import { describe, expect, it } from "vitest";
import { shouldOfferErrorReport } from "./offer";

const FLCLASH_OPEN =
  "请确认 FlClash 已打开并连接节点，且在 FlClash「设置 → 常规 → 入站」中开启了「外部控制器」";
const FLCLASH_EC =
  "检测到 FlClash 未开启外部控制器，请在 FlClash「设置 → 常规 → 入站」中开启「外部控制器」后再点「刷新节点」";
const VERGE_OPEN = "请先打开并连上【Clash Verge】，再点「刷新节点」";
const NONE_RUNNING = "未检测到正在运行的VPN软件。请先打开软件并连上节点后点「刷新节点」。";

describe("shouldOfferErrorReport", () => {
  it("hides fetch failures that are unreachable, unauthorized, or local how-to", () => {
    for (const message of [FLCLASH_OPEN, FLCLASH_EC, VERGE_OPEN, NONE_RUNNING, "连不上VPN软件。确认它已打开并连上一个可用节点后，再点「刷新节点」。"]) {
      expect(shouldOfferErrorReport({ surface: "fetch", status: "unreachable", message })).toBe(false);
    }
    expect(
      shouldOfferErrorReport({
        surface: "fetch",
        status: "unauthorized",
        message: "密钥被VPN软件拒绝。请确认代理软件外部控制器配置或重启软件后重试。",
      }),
    ).toBe(false);
    expect(
      shouldOfferErrorReport({
        surface: "fetch",
        status: "connected",
        message: "请先在上方选择软件，再点「刷新节点」。",
      }),
    ).toBe(false);
    expect(
      shouldOfferErrorReport({
        surface: "fetch",
        status: "connected",
        message:
          "检测到代理软件开启了「入站认证」，本地代理端口拒绝未认证探测。请关闭「入站认证」（FlClash 位于「设置 → 常规 → 入站」）后再点「刷新节点」。",
      }),
    ).toBe(false);
    expect(
      shouldOfferErrorReport({
        surface: "fetch",
        status: "connected",
        message:
          "检测到代理软件未开启本地混合代理端口（mixed-port 为 0）。请在软件设置中开启混合端口后再点「刷新节点」。",
      }),
    ).toBe(false);
    expect(
      shouldOfferErrorReport({
        surface: "fetch",
        status: "connected",
        message: "软件在线，但海外探测全部失败。请换个节点再试。",
      }),
    ).toBe(false);
    expect(
      shouldOfferErrorReport({
        surface: "fetch",
        status: "connected",
        message: "流量可能没走代理。请在软件里开启系统代理或 TUN 后再测。",
      }),
    ).toBe(false);
    expect(
      shouldOfferErrorReport({
        surface: "fetch",
        status: "unknown",
        message: "请先点「刷新节点」，确认已连上。",
      }),
    ).toBe(false);
  });

  it("shows fetch failures that are the app's own unexpected errors", () => {
    expect(
      shouldOfferErrorReport({
        surface: "fetch",
        status: "connected",
        message: "测全部节点时出错：Cannot read properties of undefined",
      }),
    ).toBe(true);
    expect(
      shouldOfferErrorReport({
        surface: "fetch",
        status: "connected",
        message: "TypeError: boom",
      }),
    ).toBe(true);
  });

  it("hides switch and restore failures that are only not-connected or a user abort", () => {
    for (const surface of ["switch", "restore"] as const) {
      expect(shouldOfferErrorReport({ surface, message: "当前客户端未连接" })).toBe(false);
      expect(shouldOfferErrorReport({ surface, message: "当前客户端没有连接，无法切换" })).toBe(false);
      expect(shouldOfferErrorReport({ surface, message: "Current client is not connected" })).toBe(false);
      expect(shouldOfferErrorReport({ surface, message: "已中断：收到中断信号，已停止检测。" })).toBe(false);
      expect(shouldOfferErrorReport({ surface, message: "检测被中断，结果不完整" })).toBe(false);
      expect(shouldOfferErrorReport({ surface, message: FLCLASH_OPEN })).toBe(false);
    }
  });

  it("shows unexpected switch, restore, and runner failures", () => {
    expect(shouldOfferErrorReport({ surface: "switch", message: "切换失败，没法测这个节点。" })).toBe(true);
    expect(
      shouldOfferErrorReport({ surface: "switch", message: "找不到可切换的策略组，没法单独测这个节点。" }),
    ).toBe(true);
    expect(shouldOfferErrorReport({ surface: "switch", message: "TypeError: boom" })).toBe(true);
    expect(shouldOfferErrorReport({ surface: "restore", message: "没能切回原先节点「香港 01」。" })).toBe(true);
    expect(shouldOfferErrorReport({ surface: "restore", message: "没能切回原先节点。" })).toBe(true);
    expect(
      shouldOfferErrorReport({
        surface: "runner",
        message: "Cannot read properties of undefined (reading 'port')",
      }),
    ).toBe(true);
    expect(shouldOfferErrorReport({ surface: "runner", message: "环境检查失败" })).toBe(true);
    expect(shouldOfferErrorReport({ surface: "runner", message: "当前客户端未连接" })).toBe(false);
  });

  it("shows block and warn notices, and hides info", () => {
    expect(shouldOfferErrorReport({ surface: "notice", level: "block" })).toBe(true);
    expect(shouldOfferErrorReport({ surface: "notice", level: "warn" })).toBe(true);
    expect(shouldOfferErrorReport({ surface: "notice", level: "info" })).toBe(false);
  });
});
