import { defineConfig } from "vitest/config";
// @ts-expect-error type error without @types/node package
import { readFileSync } from "node:fs";

// 与 vite.config.ts 同源：测试里 __APP_VERSION__ 也取 package.json，
// help / envelope 的版本断言因此始终对得上真实版本（不再被硬编码字面量钉死）。
const pkg = JSON.parse(
  readFileSync(new URL("./package.json", import.meta.url), "utf8"),
) as { version: string };

// Pure-logic unit tests run in node (no DOM needed). Colocated *.test.ts under src.
export default defineConfig({
  define: {
    __APP_VERSION__: JSON.stringify(pkg.version),
  },
  test: {
    environment: "node",
    include: ["src/**/*.{test,spec}.ts"],
  },
});
