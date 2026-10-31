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
  it("Windows 只保留 Clash Verge；macOS 保持 Verge + FlClash", () => {
    expect(clientOptionsFor(true).map((o) => o.id)).toEqual(["verge"]);
    expect(clientOptionsFor(false).map((o) => o.id)).toEqual(["verge", "flclash"]);
  });

  it("Windows 上存储的 flclash 归一化为 null（强制重新选择）", () => {
    expect(normalizeClientId("verge", true)).toBe("verge");
    expect(normalizeClientId("flclash", true)).toBeNull();
    expect(normalizeClientId("flclash", false)).toBe("flclash");
    // 名称仍可显示（不依赖当前平台的下拉列表）
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
