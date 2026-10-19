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
        message: "请先在上方选择软件，再点「获取节点」。",
      };
    }
    if (connection.status === "unreachable") {
      return {
        ok: false,
        message:
          "连不上VPN软件。确认它已打开并连上一个可用节点后，再点「获取节点」。",
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
        message: "请先点「获取节点」，确认已连上。",
        process: connection.message,
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
