/**
 * Client presets for Home picker → discovery / probe defaults.
 * Home labels/hints: plain 简体中文 (no geek jargon on the picker).
 * Never log secrets.
 */

import type { ControllerConfig } from "./types";

export type ClientId =
  | "verge"
  | "clashx_meta"
  | "flclash"
  | "mihomo_party"
  | "nyanpasu";

export const CLIENT_STORAGE_KEY = "clash-assist.clientId";

export type ClientOption = {
  id: ClientId;
  label: string;
  hint: string;
  /** Shown but not selectable until support is actually shipped. */
  disabled?: boolean;
};

export const CLIENT_OPTIONS: ClientOption[] = [
  {
    id: "verge",
    label: "Clash Verge / Clash Verge Rev",
    hint: "选好后点刷新，按该软件自动连接",
  },
  {
    id: "flclash",
    label: "FlClash",
    hint: "选好后点刷新，按该软件自动连接",
  },
];

const ALL_IDS: ClientId[] = [
  "verge",
  "clashx_meta",
  "flclash",
  "mihomo_party",
  "nyanpasu",
];

/** Base defaults when no client chosen yet (UI only; discovery must not run). */
export function vergeLikeDefault(): ControllerConfig {
  return {
    host: "127.0.0.1",
    port: 9097,
    secret: "",
    mixedPort: 7897,
    source: "unset-default",
    sockPath: null,
  };
}

/** Partial preset applied before discovery / probe. sockPath only when known for that client. */
export function clientPreset(id: ClientId): Partial<ControllerConfig> {
  switch (id) {
    case "verge":
      return {
        host: "127.0.0.1",
        port: 9097,
        secret: "",
        mixedPort: 7897,
        source: "preset-verge",
        // sock filled by Rust discovery (service sock / legacy / TMPDIR)
        sockPath: null,
      };
    case "clashx_meta":
      return {
        host: "127.0.0.1",
        port: 9090,
        secret: "",
        mixedPort: 7890,
        source: "preset-clashx_meta",
        sockPath: null,
      };
    case "flclash":
      return {
        host: "127.0.0.1",
        port: 9090,
        secret: "",
        mixedPort: 7890,
        source: "preset-flclash",
        sockPath: null,
      };
    case "mihomo_party":
      return {
        host: "127.0.0.1",
        port: 9090,
        secret: "",
        mixedPort: 7890,
        source: "preset-mihomo_party",
        sockPath: "/tmp/mihomo-party.sock",
      };
    case "nyanpasu":
      return {
        host: "127.0.0.1",
        port: 17650,
        secret: "",
        mixedPort: 7890,
        source: "preset-nyanpasu",
        sockPath: null,
      };
  }
}

export function isClientId(v: unknown): v is ClientId {
  return typeof v === "string" && (ALL_IDS as string[]).includes(v);
}

/**
 * Map stored clientId. Clash Verge and FlClash are selectable today.
 * Legacy clashx_meta/mihomo_party/nyanpasu/mihomo/manual/unknown → null, forcing a fresh pick.
 */
export function normalizeClientId(v: unknown): ClientId | null {
  if (v === "verge" || v === "flclash") return v;
  return null;
}

export function clientLabel(id: ClientId | null): string {
  if (!id) return "未选择";
  return CLIENT_OPTIONS.find((o) => o.id === id)?.label ?? id;
}

/** Plain-language unreachable tip naming the selected app. */
export function clientUnreachableHint(id: ClientId, source?: string): string {
  if (id === "flclash") {
    if (source?.startsWith("flclash-ec-disabled:")) {
      return "检测到 FlClash 未开启外部控制器，请在 FlClash「设置 → 常规 → 入站」中开启「外部控制器」后再点「获取节点」";
    }
    return "请确认 FlClash 已打开并连接节点，且在 FlClash「设置 → 常规 → 入站」中开启了「外部控制器」";
  }
  const name = clientLabel(id);
  return `请先打开并连上【${name}】，再点「获取节点」`;
}

/** Legacy Verge unix sock (pre-2.5.6). Prefer Rust discovery for live path. */
export const VERGE_SOCK_LEGACY = "/tmp/verge/verge-mihomo.sock";
/** @deprecated Use VERGE_SOCK_LEGACY; live path comes from discover_mihomo. */
export const VERGE_SOCK = VERGE_SOCK_LEGACY;
/** Clash Verge 2.5.6+ service sock dir: .../users/<uid>/verge-mihomo.sock */
export const VERGE_SERVICE_USERS_DIR = "/var/run/clash-verge-service/users";
export const MIHOMO_PARTY_SOCK = "/tmp/mihomo-party.sock";

/** Ordered Verge sock candidates (align with Rust `platform::verge_sock_candidates`). */
export function vergeSockCandidates(uid?: number | null): string[] {
  const out: string[] = [];
  const push = (s: string) => {
    if (s && !out.includes(s)) out.push(s);
  };
  if (typeof uid === "number" && Number.isFinite(uid)) {
    push(`${VERGE_SERVICE_USERS_DIR}/${uid}/verge-mihomo.sock`);
  }
  push(VERGE_SOCK_LEGACY);
  // TMPDIR is not available in the webview the same way; Rust discovery expands it.
  return out;
}
