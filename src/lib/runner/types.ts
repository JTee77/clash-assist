import type { CheckCard, EgressReport, ExitIpInfo } from "../egress/types";
import type { ConnectionState, ProxyNode } from "../mihomo";
import type { GateResult, NodeScoreResult } from "../score";

export type RunnerProgress = {
  text: string;
  current?: number;
  total?: number;
  testingNode?: string;
  /** 机器可读阶段（CLI NDJSON 进度用；GUI 只看 text） */
  stage?: "precheck" | "switch" | "test" | "restore";
};

/** Shared UI hooks — no React; HomePage wires setState here. */
export type RunnerHooks = {
  onProgress: (p: RunnerProgress | null) => void;
  onNodeCards: (cards: CheckCard[]) => void;
  onUpsertNodeCard?: (card: CheckCard) => void;
  onReport?: (r: EgressReport) => void;
  onUpsertScore?: (score: NodeScoreResult) => void;
  onScores?: (scores: NodeScoreResult[]) => void;
  onHint?: (msg: string | null) => void;
  onRestoreError?: (msg: string | null) => void;
  onGate?: (g: GateResult) => void;
  /** Env runner */
  onEnvCards?: (cards: CheckCard[]) => void;
  onUpsertEnvCard?: (card: CheckCard) => void;
};

export type TestContext = {
  connection: ConnectionState;
  forceMock: boolean;
  mixedPort: number | null;
  ensureGate: () => Promise<GateResult>;
};

export type TestAllContext = TestContext & {
  nodes: ProxyNode[];
  shouldAbort: () => boolean;
};

export type TestOneContext = TestContext & {
  node: ProxyNode;
};

export type RunEnvContext = {
  connection: ConnectionState;
  mixedPort: number | null;
  exitIp?: ExitIpInfo | null;
};
