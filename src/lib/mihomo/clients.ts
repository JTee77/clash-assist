/**
 * Client presets for Home picker → discovery / probe defaults.
 * Home labels/hints: plain 简体中文 (no geek jargon on the picker).
 * Never log secrets.
 */

import { isWindows } from "../platform";
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

const ALL_CLIENT_OPTIONS: ClientOption[] = [
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

/** Clash Verge 与 FlClash 在 macOS 和 Windows 上都可选。 */
export function clientOptionsFor(windows: boolean): ClientOption[] {
  void windows;
  return ALL_CLIENT_OPTIONS;
}

export const CLIENT_OPTIONS: ClientOption[] = clientOptionsFor(isWindows());

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
        sockPath: mihomoPartySock(),
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
 * Map stored clientId. Clash Verge and FlClash are selectable on macOS and Windows.
 * Legacy clashx_meta/mihomo_party/nyanpasu/mihomo/manual/unknown → null, forcing a fresh pick.
 */
export function normalizeClientId(v: unknown, windows: boolean = isWindows()): ClientId | null {
  void windows;
  if (v === "verge" || v === "flclash") return v;
  return null;
}

export function clientLabel(id: ClientId | null): string {
  if (!id) return "未选择";
  return ALL_CLIENT_OPTIONS.find((o) => o.id === id)?.label ?? id;
}

const FLCLASH_EC_DISABLED =
  "检测到 FlClash 未开启外部控制器，请在 FlClash「设置 → 常规 → 入站」中开启「外部控制器」后再点「刷新节点」";
const FLCLASH_EC_REMOTE =
  "FlClash 的外部控制器不是本机地址，已拒绝连接。请把它改成 127.0.0.1 后再点「刷新节点」。";
const FLCLASH_MISSING =
  "未检测到 FlClash 的配置。请先安装并打开 FlClash，在「设置 → 常规 → 入站」开启「外部控制器」后再点「刷新节点」。";
const FLCLASH_NOT_RUNNING =
  "已找到 FlClash 的配置，但外部控制器没有在监听。请先打开 FlClash 并连上节点后再点「刷新节点」。";
const FLCLASH_IMAGE =
  "监听该端口的程序不是 FlClash 内核（FlClashCore），已拒绝连接。请确认「外部控制器」端口没有被其他程序占用，并在 FlClash 中重启后再点「刷新节点」。";
const FLCLASH_GENERAL =
  "请确认 FlClash 已打开并连接节点，且在 FlClash「设置 → 常规 → 入站」中开启了「外部控制器」";

function flclashBase(source?: string): string {
  const base = (source ?? "").split(";")[0] ?? "";
  return base;
}

/** Plain-language unreachable tip naming the selected app. */
export function clientUnreachableHint(id: ClientId, source?: string): string {
  if (id === "flclash") return flclashLocalMessage(source);
  const name = clientLabel(id);
  return `请先打开并连上【${name}】，再点「刷新节点」`;
}

/**
 * FlClash 连不上时的本地操作提示。
 * `detail` 是控制器拒绝原因（例如 Windows 监听者校验的 image-mismatch），不含密钥。
 */
export function flclashLocalMessage(source?: string, detail?: string | null): string {
  if (detail && /image-mismatch/i.test(detail)) return FLCLASH_IMAGE;
  const base = flclashBase(source);
  if (base.startsWith("flclash-ec-disabled")) return FLCLASH_EC_DISABLED;
  if (base.startsWith("flclash-ec-remote")) return FLCLASH_EC_REMOTE;
  if (
    !base ||
    base === "defaults" ||
    base === "flclash:defaults" ||
    base.startsWith("preset-")
  ) {
    return FLCLASH_MISSING;
  }
  if (base.startsWith("flclash:")) return FLCLASH_NOT_RUNNING;
  return FLCLASH_GENERAL;
}

/** Legacy Verge unix sock (pre-2.5.6). Prefer Rust discovery for live path. */
export const VERGE_SOCK_LEGACY = "/tmp/verge/verge-mihomo.sock";
/** @deprecated Use VERGE_SOCK_LEGACY; live path comes from discover_mihomo. */
export const VERGE_SOCK = VERGE_SOCK_LEGACY;
/** Clash Verge 2.5.6+ service sock dir: .../users/<uid>/verge-mihomo.sock */
export const VERGE_SERVICE_USERS_DIR = "/var/run/clash-verge-service/users";
export const MIHOMO_PARTY_SOCK = "/tmp/mihomo-party.sock";

/** Mihomo Party 的 unix sock；Windows 没有 unix sock 候选（控制管道由 Rust discovery 给出）。 */
export function mihomoPartySock(windows: boolean = isWindows()): string | null {
  return windows ? null : MIHOMO_PARTY_SOCK;
}

/** Ordered Verge sock candidates (align with Rust `platform::verge_sock_candidates`). Windows: none. */
export function vergeSockCandidates(uid?: number | null, windows: boolean = isWindows()): string[] {
  const out: string[] = [];
  if (windows) return out;
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
