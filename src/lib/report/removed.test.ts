import { describe, expect, it } from "vitest";

const sources = import.meta.glob(
  ["../../**/*.{ts,tsx}", "../../../src-tauri/src/**/*.rs", "!../../**/*.test.ts"],
  { query: "?raw", import: "default", eager: true },
) as Record<string, string>;

const blob = Object.entries(sources)
  .map(([path, text]) => `\n@@ ${path}\n${text}`)
  .join("\n");

describe("export and share are gone", () => {
  it("has no export or share entry points left in the app", () => {
    expect(Object.keys(sources).some((p) => p.includes("shareCard"))).toBe(false);
    for (const gone of [
      "save_png_document",
      "save_text_document",
      "renderSharePngBase64",
      "shareLines",
      "导出诊断报告",
      "分享结果",
      "report.export",
      "report.share",
      "savePngDocument",
      "saveTextDocument",
      "home-report-row",
    ]) {
      expect(blob, gone).not.toContain(gone);
    }
  });

  it("keeps preview-then-send and the GET follow", () => {
    expect(blob).toContain("feedback_get");
    expect(blob).toContain("feedback_post");
    expect(blob).toContain("发送这份错误报告");
    expect(blob).toContain("report.previewHint");
    expect(blob).toContain("getOnceViaTauri");
    expect(blob).toContain("distinctMessages");
    expect(blob).toContain("shouldOfferErrorReport");
  });
});
