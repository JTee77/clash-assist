import { invoke } from "@tauri-apps/api/core";
import type { GetOnce, PostOnce } from "./sendClient";

export type FeedbackExchange = {
  status: number;
  location: string | null;
  body: string;
};

export type HostInfo = {
  os: string;
  arch: string;
};

/** 单次 POST。Rust 侧关闭了自动重定向。 */
export const postOnceViaTauri: PostOnce = async (url, body) => {
  const res = await invoke<FeedbackExchange>("feedback_post", { url, body });
  return { status: res.status, location: res.location, body: res.body };
};

/** 单次 GET，不带正文。用于 Apps Script 302/303 之后读取 echo 结果。 */
export const getOnceViaTauri: GetOnce = async (url) => {
  const res = await invoke<FeedbackExchange>("feedback_get", { url });
  return { status: res.status, location: res.location, body: res.body };
};

export function reportHostInfo(): Promise<HostInfo> {
  return invoke<HostInfo>("report_host_info");
}
