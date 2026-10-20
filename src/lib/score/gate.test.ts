import { describe, expect, it, vi, beforeEach } from "vitest";

const h = vi.hoisted(() => ({
  checkReachability: vi.fn(),
  checkBareEgress: vi.fn(),
}));

vi.mock("../egress/diagnostics", () => ({
  checkReachability: h.checkReachability,
  checkBareEgress: h.checkBareEgress,
}));

import { runLightGate } from "./gate";
import type { ConnectionState } from "../mihomo/types";

function makeConnected(overrides: Partial<ConnectionState["config"]> = {}): ConnectionState {
  return {
    status: "connected",
    message: "已连上 FlClash",
    config: {
      host: "127.0.0.1",
      port: 9090,
      secret: "",
      mixedPort: 7890,
      source: "flclash",
      ...overrides,
    },
    currentProxy: "🇭🇰 香港 01",
    usingMock: false,
  };
}

describe("runLightGate defensive checks (0.2.4)", () => {
  beforeEach(() => {
    h.checkReachability.mockReset();
    h.checkBareEgress.mockReset();
  });

  it("blocks immediately with clear guidance when inboundAuth is enabled", async () => {
    const res = await runLightGate(makeConnected({ inboundAuth: true }));
    expect(res.ok).toBe(false);
    expect(res.message).toContain("入站认证");
    expect(res.message).toContain("设置 → 常规 → 入站");
    expect(h.checkReachability).not.toHaveBeenCalled();
    expect(h.checkBareEgress).not.toHaveBeenCalled();
  });

  it("blocks immediately with clear guidance when mixedPortDisabled is true", async () => {
    const res = await runLightGate(
      makeConnected({ mixedPort: 0, mixedPortDisabled: true }),
    );
    expect(res.ok).toBe(false);
    expect(res.message).toContain("mixed-port 为 0");
    expect(h.checkReachability).not.toHaveBeenCalled();
    expect(h.checkBareEgress).not.toHaveBeenCalled();
  });

  it("proceeds to reachability and bare-egress checks when config is healthy", async () => {
    h.checkReachability.mockResolvedValue({
      id: "reach",
      title: "连通性",
      level: "ok",
      conclusion: "正常",
      process: "ok",
    });
    h.checkBareEgress.mockResolvedValue({
      id: "bare",
      title: "代理路径",
      level: "ok",
      conclusion: "正常",
      process: "ok",
    });

    const res = await runLightGate(makeConnected());
    expect(res.ok).toBe(true);
    expect(h.checkReachability).toHaveBeenCalledWith(7890);
    expect(h.checkBareEgress).toHaveBeenCalledWith(7890);
  });
});
