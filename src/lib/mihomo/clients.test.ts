import { describe, expect, it } from "vitest";
import {
  clientLabel,
  clientOptionsFor,
  MIHOMO_PARTY_SOCK,
  mihomoPartySock,
  normalizeClientId,
  vergeSockCandidates,
} from "./clients";

describe("Windows 客户端列表与 sock 候选（windows-plan §2.7）", () => {
  it("Windows 与 macOS 都提供 Clash Verge 和 FlClash", () => {
    expect(clientOptionsFor(true).map((o) => o.id)).toEqual(["verge", "flclash"]);
    expect(clientOptionsFor(false).map((o) => o.id)).toEqual(["verge", "flclash"]);
  });

  it("Windows 上存储的 flclash 仍可选", () => {
    expect(normalizeClientId("verge", true)).toBe("verge");
    expect(normalizeClientId("flclash", true)).toBe("flclash");
    expect(normalizeClientId("flclash", false)).toBe("flclash");
    expect(clientLabel("flclash")).toBe("FlClash");
  });

  it("Windows 不产生 unix sock 候选", () => {
    expect(mihomoPartySock(true)).toBeNull();
    expect(mihomoPartySock(false)).toBe(MIHOMO_PARTY_SOCK);
    expect(vergeSockCandidates(501, true)).toEqual([]);
    expect(vergeSockCandidates(501, false)).toEqual([
      "/var/run/clash-verge-service/users/501/verge-mihomo.sock",
      "/tmp/verge/verge-mihomo.sock",
    ]);
  });
});
