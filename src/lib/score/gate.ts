import { checkBareEgress, checkReachability } from "../egress/diagnostics";
import type { ConnectionState } from "../mihomo/types";
import type { GateResult } from "./types";

/**
 * 轻量自动门槛：客户端是否连上、隧道是否大致工作、是否明显未走代理直连。
 * 失败则口语阻断，不进入节点测评。
 */
export async function runLightGate(connection: ConnectionState): Promise<GateResult> {
  if (!connection.usingMock) {
    if (connection.status === "unknown") {
      return {
        ok: false,
        message: "请先在上方选择软件，再点「刷新节点」。",
      };
    }
    if (connection.status === "unreachable") {
      const specificMsg = connection.message?.trim();
      const hasSpecificHint =
        specificMsg &&
        (specificMsg.includes("未检测到正在运行的VPN软件") ||
          specificMsg.includes("检测到 FlClash 未开启外部控制器") ||
          specificMsg.includes("请确认 FlClash 已打开") ||
          specificMsg.includes("请先打开并连上【"));
      return {
        ok: false,
        message: hasSpecificHint
          ? specificMsg
          : "连不上VPN软件。确认它已打开并连上一个可用节点后，再点「刷新节点」。",
        process: connection.message,
      };
    }
    if (connection.status === "unauthorized") {
      return {
        ok: false,
        message: "密钥被VPN软件拒绝。请确认代理软件外部控制器配置或重启软件后重试。",
        process: connection.proxiesError ?? connection.message,
      };
    }
    if (connection.status !== "connected" && connection.status !== "mock") {
      return {
        ok: false,
        message: "请先点「刷新节点」，确认已连上。",
        process: connection.message,
      };
    }
    if (connection.config?.inboundAuth) {
      return {
        ok: false,
        message:
          "检测到代理软件开启了「入站认证」，本地代理端口拒绝未认证探测。请关闭「入站认证」（FlClash 位于「设置 → 常规 → 入站」）后再点「刷新节点」。",
        process:
          "GET /configs 检测到 authentication 非空且未豁免 127.0.0.1 环回地址。",
      };
    }
    if (connection.config?.mixedPortDisabled) {
      return {
        ok: false,
        message:
          "检测到代理软件未开启本地混合代理端口（mixed-port 为 0）。请在软件设置中开启混合端口后再点「刷新节点」。",
        process:
          "GET /configs 返回 mixed-port=0 且 port=0，无可用本地 HTTP/Mixed 代理端口。",
      };
    }
  }

  const mixedPort = connection.config?.mixedPort ?? null;
  const reach = await checkReachability(mixedPort);
  if (reach.level === "fail") {
    return {
      ok: false,
      message:
        "软件在线，但海外探测全部失败。请换个节点再试。",
      process: reach.process,
    };
  }

  const bare = await checkBareEgress(mixedPort);
  if (bare.level === "warn" && /可能未走代理直连|代理路径失败但直连/.test(bare.conclusion)) {
    return {
      ok: false,
      message:
        "流量可能没走代理。请在软件里开启系统代理或 TUN 后再测。",
      process: bare.process,
    };
  }

  return {
    ok: true,
    message: "测试条件满足：客户端在线，访问海外站点大致正常，未见明显未走代理直连。可以开始测节点。",
    process: [reach.conclusion, bare.conclusion].join("\n"),
  };
}
