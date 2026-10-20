/** Mihomo / Clash Meta API types */

export interface ControllerConfig {
  host: string;
  port: number;
  secret: string;
  mixedPort: number;
  source: string;
  sockPath?: string | null;
  /** True when GET /configs has non-empty authentication without 127.0.0.1 loopback exemption */
  inboundAuth?: boolean;
  /** True when GET /configs explicitly reports mixed-port=0 and port=0 */
  mixedPortDisabled?: boolean;
}

/**
 * Per-node capability flags from mihomo `/proxies`. This is the full six-flag
 * vocabulary the core can emit; whether a given subscription lights any of them
 * depends on its node configs, so none is assumed always-on. `undefined` means
 * the field was absent (treated as false when rendering chips).
 */
export interface NodeCapabilities {
  udp?: boolean;
  xudp?: boolean;
  uot?: boolean;
  tfo?: boolean;
  smux?: boolean;
  mptcp?: boolean;
}

export interface ProxyInfo extends NodeCapabilities {
  name: string;
  type: string;
  /** mihomo health flag; absent in some payloads. */
  alive?: boolean;
  history?: { time: string; delay: number }[];
  all?: string[];
  now?: string;
}

export interface ProxyNode extends NodeCapabilities {
  name: string;
  type: string;
  region: string;
  raw: ProxyInfo;
  /** Client-reported health: mihomo `alive` flag (undefined = absent). */
  alive?: boolean;
  /**
   * Client's most recent delay-test result (ms); 0 = that test failed.
   * undefined = the client has never tested this node (not the same as dead).
   */
  lastDelay?: number;
  /** ISO-8601 time of that last client test (freshness check for skip logic). */
  lastDelayAt?: string;
}

export type ConnectionStatus =
  | "unknown"
  | "connected"
  | "unreachable"
  | "unauthorized"
  | "mock";

export interface ConnectionState {
  status: ConnectionStatus;
  message: string;
  config: ControllerConfig | null;
  currentProxy: string | null;
  usingMock: boolean;
  /** Proxies fetch / filter error when connected but nodes empty or failed */
  proxiesError?: string | null;
}

export interface DelayResult {
  name: string;
  region: string;
  proto: string;
  avgDelay: number;
  jitter: number;
  lossRate: number;
  alive: boolean;
}

export type NodeTestMode = "quick" | "deep" | "topN" | "aiOnly";

export interface NodeTestPlan {
  mode: NodeTestMode;
  label: string;
  description: string;
  implemented: boolean;
}

export const NODE_TEST_PLANS: NodeTestPlan[] = [
  {
    mode: "quick",
    label: "快速延迟",
    description: "多轮 Google/CF generate_204 延迟、抖动、丢包（低流量）",
    implemented: true,
  },
  {
    mode: "deep",
    label: "深度测速",
    description: "全节点下载带宽 + TTFB + AI/IP（高流量，v1 占位）",
    implemented: false,
  },
  {
    mode: "topN",
    label: "优选 Top-N",
    description: "筛选低延迟节点后测真实带宽与解锁（v1 占位）",
    implemented: false,
  },
  {
    mode: "aiOnly",
    label: "AI 专项",
    description: "批量 Gemini / ChatGPT / IP 类型（v1 占位）",
    implemented: false,
  },
];

export const IGNORE_PROXY_TYPES = new Set([
  "Selector",
  "URLTest",
  "Fallback",
  "LoadBalance",
  "Relay",
  "Direct",
  "Reject",
  "Compatible",
  "Pass",
]);

export const JUNK_NAME_KEYWORDS = ["剩余", "到期", "官网"];
