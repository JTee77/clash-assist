import { describe, expect, it } from "vitest";
import { distinctMessages } from "./messages";

const FLCLASH =
  "请确认 FlClash 已打开并连接节点，且在 FlClash「设置 → 常规 → 入站」中开启了「外部控制器」";

describe("distinctMessages", () => {
  it("keeps one copy when the gate message and process repeat the same how-to", () => {
    expect(distinctMessages([FLCLASH, FLCLASH])).toBe(FLCLASH);
    expect(distinctMessages([`${FLCLASH}\n`, `\n${FLCLASH}`])).toBe(FLCLASH);
  });

  it("keeps a second distinct line", () => {
    expect(distinctMessages([FLCLASH, `${FLCLASH}\nGET /version 超时`])).toBe(
      `${FLCLASH}\nGET /version 超时`,
    );
  });

  it("drops blank parts", () => {
    expect(distinctMessages(["", "  ", FLCLASH])).toBe(FLCLASH);
  });
});
