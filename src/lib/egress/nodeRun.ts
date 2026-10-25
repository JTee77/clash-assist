/**
 * 节点向检测：连通性、出口、延迟/带宽抽样、流媒体与 AI 粗检。
 * 不跑 DNS / IPv6 / WebRTC / 分流 / 直连旁路等环境项。
 */
import type { ControllerConfig } from "../mihomo/types";
import {
  checkAppStoreUnlock,
  checkDisneyUnlock,
  checkGooglePlayUnlock,
  checkNetflixUnlock,
  checkPrimeVideoUnlock,
  checkReachability,
  checkSpotifyUnlock,
  checkTikTokUnlock,
  checkYoutubeUnlock,
  exitIpCard,
  fetchExitIp,
  probeChatgptUnlock,
  probeGeminiUnlock,
  sampleBandwidth,
  sampleLatency,
  timeoutCard,
  unlockCard,
  withCardDeadline,
  withFailRetryUnlock,
} from "./diagnostics";
import type { CheckCard, EgressReport, UnlockResult } from "./types";
import { mapPool } from "./pool";
export { mapPool } from "./pool";

export const NODE_CARD_IDS = [
  "reachability",
  "exit-ip",
  "latency",
  "bandwidth",
  "gemini",
  "chatgpt",
  "netflix",
  "disney",
  "youtube",
  "tiktok",
  "spotify",
  "prime-video",
  "app-store",
  "google-play",
] as const;

export type NodeRunOptions = {
  mixedPort?: number | null;
  mihomoConfig?: ControllerConfig | null;
};

/** 针对「当前出口」的节点向深测（不切换节点）。 */
export async function runNodeDiagnostics(
  onCard?: (card: CheckCard) => void,
  options?: NodeRunOptions,
): Promise<EgressReport> {
  const mixedPort = options?.mixedPort ?? null;
  const note =
    mixedPort != null && mixedPort > 0
      ? "只测当前出口相关项（连通、出口、带宽与服务检查）。环境项请用「环境泄漏检查」。"
      : "未配置代理口时，部分探针可能走窗口直连。建议先刷新节点。";

  const push = (c: CheckCard) => {
    onCard?.(c);
    return c;
  };

  const reach = push(
    await withCardDeadline(
      "连通性",
      "reachability",
      (signal) => checkReachability(mixedPort, { signal }),
      14000,
    ),
  );

  const exitAc = new AbortController();
  let exitTimer: ReturnType<typeof setTimeout> | undefined;
  let exit: Awaited<ReturnType<typeof fetchExitIp>>;
  try {
    exit = await Promise.race([
      fetchExitIp(mixedPort, exitAc.signal),
      new Promise<Awaited<ReturnType<typeof fetchExitIp>>>((resolve) => {
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
  const exitCardResult = push(exitIpCard(exit));

  let gemini: UnlockResult = {
    supported: false,
    level: "unknown",
    region: null,
    status: "未完成",
  };
  let chatgpt: UnlockResult = {
    supported: false,
    level: "unknown",
    region: null,
    status: "未完成",
  };
  let latencyMs: number | null = null;

  type Job = {
    id: string;
    title: string;
    deadlineMs: number;
    run: (signal: AbortSignal) => Promise<CheckCard>;
  };

  const jobs: Job[] = [
    {
      id: "gemini",
      title: "Gemini",
      deadlineMs: 16000,
      run: async (signal) => {
        gemini = await withFailRetryUnlock(() => probeGeminiUnlock(mixedPort, signal), signal);
        return unlockCard("gemini", "Gemini", gemini);
      },
    },
    {
      id: "chatgpt",
      title: "ChatGPT",
      deadlineMs: 22000,
      run: async (signal) => {
        chatgpt = await withFailRetryUnlock(() => probeChatgptUnlock(mixedPort, signal), signal);
        return unlockCard("chatgpt", "ChatGPT", chatgpt);
      },
    },
    {
      id: "latency",
      title: "延迟采样",
      deadlineMs: 22000,
      run: async (signal) => {
        const { ms, card: c } = await sampleLatency(mixedPort, signal);
        latencyMs = ms;
        return c;
      },
    },
    {
      id: "bandwidth",
      title: "抽样带宽",
      deadlineMs: 42000,
      run: (signal) => sampleBandwidth(mixedPort, { mode: "full", signal }),
    },
    {
      id: "netflix",
      title: "Netflix",
      deadlineMs: 16000,
      run: (signal) => checkNetflixUnlock(mixedPort, signal),
    },
    {
      id: "disney",
      title: "Disney+",
      deadlineMs: 16000,
      run: (signal) => checkDisneyUnlock(mixedPort, signal),
    },
    {
      id: "youtube",
      title: "YouTube Premium",
      deadlineMs: 16000,
      run: (signal) => checkYoutubeUnlock(mixedPort, signal),
    },
    {
      id: "tiktok",
      title: "TikTok",
      deadlineMs: 16000,
      run: (signal) => checkTikTokUnlock(mixedPort, signal),
    },
    {
      id: "spotify",
      title: "Spotify",
      deadlineMs: 16000,
      run: (signal) => checkSpotifyUnlock(mixedPort, signal),
    },
    {
      id: "prime-video",
      title: "Prime Video",
      deadlineMs: 16000,
      run: (signal) => checkPrimeVideoUnlock(mixedPort, signal),
    },
    {
      id: "app-store",
      title: "App Store",
      deadlineMs: 16000,
      run: (signal) => checkAppStoreUnlock(mixedPort, signal),
    },
    {
      id: "google-play",
      title: "Google Play",
      deadlineMs: 16000,
      run: (signal) => checkGooglePlayUnlock(mixedPort, signal),
    },
  ];

  const settled = await mapPool(jobs, 3, async (job) => {
    const c = await withCardDeadline(job.title, job.id, job.run, job.deadlineMs);
    return push(c);
  });

  const byId = new Map(settled.map((c) => [c.id, c]));
  const cards = NODE_CARD_IDS.map((id) => {
    if (id === "reachability") return reach;
    if (id === "exit-ip") return exitCardResult;
    return byId.get(id) ?? timeoutCard(id, id);
  });

  return {
    ranAt: new Date().toISOString(),
    cards,
    exitIp: exit,
    gemini,
    chatgpt,
    latencyMs,
    note,
  };
}
