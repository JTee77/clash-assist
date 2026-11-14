import { invoke } from "@tauri-apps/api/core";
import type { PostOnce } from "./sendClient";

export type FeedbackExchange = {
  status: number;
  location: string | null;
  body: string;
};

export type HostInfo = {
  os: string;
  arch: string;
};

/** 单次 POST。Rust 侧关闭了自动重定向，跳转由 sendClient 继续 POST。 */
export const postOnceViaTauri: PostOnce = async (url, body) => {
  const res = await invoke<FeedbackExchange>("feedback_post", { url, body });
  return { status: res.status, location: res.location, body: res.body };
};

export function saveTextDocument(
  defaultName: string,
  content: string,
  extension: string,
): Promise<string | null> {
  return invoke<string | null>("save_text_document", { defaultName, content, extension });
}

export type SavedPng = {
  path: string;
  /** `written` 已写入文字摘要；`kept` 同名摘要已存在，用户选择不覆盖。 */
  sidecar: "written" | "kept";
};

export function savePngDocument(
  defaultName: string,
  pngBase64: string,
  sidecarText: string,
): Promise<SavedPng | null> {
  return invoke<SavedPng | null>("save_png_document", {
    defaultName,
    pngBase64,
    sidecarText,
  });
}

export function reportHostInfo(): Promise<HostInfo> {
  return invoke<HostInfo>("report_host_info");
}
