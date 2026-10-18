/**
 * 环境向检测：DNS / IPv6 / WebRTC / 分流 / 直连旁路。
 * 作为第二入口「环境泄漏检查」，不默认每次强跑。
 */
import type { ControllerConfig } from "../mihomo/types";
import {
  checkBareEgress,
  checkDnsLeakApproach,
  checkDnsResolvers,
  checkIpv6Leak,
  checkSplitRouting,
  checkWebRtcLeak,
  fetchExitIp,
  fetchIpCountry,
  probeDirectV4,
  probeV4Via,
  withCardDeadline,
} from "./diagnostics";
import { localeCountry } from "./leakMatrix";
import type { CheckCard, ExitIpInfo } from "./types";

export const ENV_CARD_IDS = [
  "bare-egress",
  "dns-leak",
  "ipv6-leak",
  "webrtc",
  "split-routing",
] as const;

export type EnvRunOptions = {
  mixedPort?: number | null;
  mihomoConfig?: ControllerConfig | null;
  /** 若已有出口信息可传入，避免重复拉取 */
  exitIp?: ExitIpInfo | null;
};

export async function runEnvDiagnostics(
  onCard?: (card: CheckCard) => void,
  options?: EnvRunOptions,
): Promise<CheckCard[]> {
  const mixedPort = options?.mixedPort ?? null;
  const mihomoConfig = options?.mihomoConfig ?? null;
  const push = (c: CheckCard) => {
    onCard?.(c);
    return c;
  };

  let exit = options?.exitIp ?? null;
  if (!exit) {
    const exitAc = new AbortController();
    let exitTimer: ReturnType<typeof setTimeout> | undefined;
    try {
      exit = await Promise.race([
        fetchExitIp(mixedPort, exitAc.signal),
        new Promise<ExitIpInfo>((resolve) => {
          exitTimer = setTimeout(() => {
            exitAc.abort();
            resolve({
              ip: null,
              country: null,
              countryCode: null,
              org: null,
              isp: null,
              hosting: null,
              ipTypeLabel: "--",
            });
          }, 5000);
        }),
      ]);
    } finally {
      if (exitTimer) clearTimeout(exitTimer);
    }
  }

  // 真实归属参照（归属地判定的锚点）：优先实测直连出口——
  //  · v4 直连与 v4 代理两路不同源（系统代理等非 TUN 场景）→ 直连出口就是真实 ISP，
  //    确定性参照
  //  · v4 两路同源（TUN 全局接管）→ 机器上不存在可见的真实出口，退回系统区域启发式；
  //    此时 DNS 结构上也到不了真实 ISP，启发式只承担次要角色
  //  注意必须 v4-vs-v4 同族比较：出口 IP 探测可能返回节点的 v6（与 v4 直连必然
  //  不相等），跨族字符串比对会把 TUN 误判成"非 TUN"（v0.1.11 实测踩坑）。
  const expectProxy = mixedPort != null && mixedPort > 0;
  let realCountry = localeCountry();
  if (expectProxy) {
    const [directV4, proxiedV4] = await Promise.all([
      probeDirectV4(),
      probeV4Via(mixedPort, 4500),
    ]);
    if (directV4 && proxiedV4 && directV4 !== proxiedV4) {
      const cc = await fetchIpCountry(directV4, mixedPort);
      if (cc) realCountry = cc;
    }
  }

  const jobs: {
    id: string;
    title: string;
    deadlineMs: number;
    run: (signal: AbortSignal) => Promise<CheckCard>;
  }[] = [
    {
      // 按严重程度排序：直连旁路是唯一"整机裸奔"级的失败，放在最前
      id: "bare-egress",
      title: "直连旁路检查",
      deadlineMs: 14000,
      run: (signal) => checkBareEgress(mixedPort, signal),
    },
    {
      id: "dns-leak",
      title: "DNS 解析器",
      deadlineMs: 12000,
      run: async (signal) => {
        // 优先系统解析器；失败时仍给出启发式对照
        try {
          return await checkDnsResolvers(exit!, mixedPort, realCountry, signal);
        } catch {
          return checkDnsLeakApproach(exit!, mixedPort);
        }
      },
    },
    {
      id: "ipv6-leak",
      title: "IPv6 泄漏",
      deadlineMs: 12000,
      run: (signal) => checkIpv6Leak(mixedPort, signal),
    },
    {
      id: "webrtc",
      title: "WebRTC",
      deadlineMs: 10000,
      run: (signal) => checkWebRtcLeak(exit, mixedPort, realCountry, signal),
    },
    {
      id: "split-routing",
      title: "分流检查",
      deadlineMs: 22000,
      run: (signal) => checkSplitRouting(mixedPort, mihomoConfig, signal),
    },
  ];

  const cards: CheckCard[] = [];
  for (const job of jobs) {
    const c = await withCardDeadline(job.title, job.id, job.run, job.deadlineMs);
    cards.push(push(c));
  }
  return cards;
}
