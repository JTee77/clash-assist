/**
 * update-download-progress 事件名 Rust↔TS 契约测试。
 * stream_dmg_to_file（Rust）广播的事件名必须与 useUpdateFlow（TS）监听的名字逐字一致，
 * 否则进度百分比静默失效（不报错、只在真实下载时可感知）——用 ?raw 直读双侧源码钉死。
 * M3：监听从 TitleBar 迁到 useUpdateFlow（TitleBar / UpdateBar 共用）。
 * 模式同 cli.test.ts 直读 lib.rs 断言 CLI_VERBS。
 */
import { describe, expect, it } from "vitest";
import rustUpdateCheck from "../../src-tauri/src/update_check.rs?raw";
import updateFlow from "../hooks/useUpdateFlow.ts?raw";

const rustEventName = rustUpdateCheck.match(
  /DOWNLOAD_PROGRESS_EVENT:\s*&str\s*=\s*"([^"]+)"/,
)?.[1];
const tsListenNames = [...updateFlow.matchAll(/listen<[^>]*>\(\s*"([^"]+)"/g)].map(
  (m) => m[1],
);

describe("update-download-progress 事件名契约", () => {
  it("Rust 广播名与 useUpdateFlow 监听名一致", () => {
    expect(rustEventName).toBeTruthy();
    expect(tsListenNames.length).toBeGreaterThan(0);
    for (const name of tsListenNames) {
      expect(name).toBe(rustEventName);
    }
  });
});
