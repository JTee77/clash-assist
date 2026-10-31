import { describe, expect, it } from "vitest";
import { hasClientFootprint } from "./useConnection";
import type { ConnectionState } from "../lib/mihomo";

function withSource(source: string | undefined): ConnectionState {
  return {
    status: "unreachable",
    message: "",
    config:
      source === undefined
        ? null
        : { host: "127.0.0.1", port: 9097, secret: "", mixedPort: 7897, source, sockPath: null },
    currentProxy: null,
    usingMock: false,
    proxiesError: null,
  } as ConnectionState;
}

describe("hasClientFootprint", () => {
  it("真实配置 / 管道 / TCP 来源算已安装", () => {
    expect(hasClientFootprint(withSource("verge-config:C:/v/config.yaml"))).toBe(true);
    expect(hasClientFootprint(withSource("verge-pipe:verge-mihomo"))).toBe(true);
    expect(hasClientFootprint(withSource("verge-tcp(listener-verified):C:/v/config.yaml"))).toBe(true);
    expect(hasClientFootprint(withSource("verge-pipe-untrusted:C:/v/config.yaml"))).toBe(true);
  });

  it("默认回退不算已安装", () => {
    expect(hasClientFootprint(withSource(undefined))).toBe(false);
    expect(hasClientFootprint(withSource(""))).toBe(false);
    expect(hasClientFootprint(withSource("defaults"))).toBe(false);
    expect(hasClientFootprint(withSource("unset-default"))).toBe(false);
    expect(hasClientFootprint(withSource("flclash:defaults"))).toBe(false);
    expect(hasClientFootprint(withSource("preset-verge"))).toBe(false);
  });

  it("只看去掉后缀的标签：可信管道 b 带着「a 被拒」的后缀仍算已安装（PR52 Ellie must-fix 2）", () => {
    expect(hasClientFootprint(withSource("verge-pipe:b;pipe-insecure-dacl=a"))).toBe(true);
    expect(hasClientFootprint(withSource("verge-config:C:/v/config.yaml;pipe-insecure-dacl=a|b"))).toBe(true);
    expect(
      hasClientFootprint(withSource("verge-tcp(listener-unverified,pipe-insecure):C:/v/config.yaml")),
    ).toBe(true);
    expect(
      hasClientFootprint(withSource("verge-pipe:b;ipv6-dual-stack=9097;pipe-open-to-all=b;future=1")),
    ).toBe(true);
    // 坏编码的后缀不影响判断
    expect(hasClientFootprint(withSource("verge-pipe:b;pipe-insecure-dacl=%zz"))).toBe(true);
  });

  it("defaults / preset 来源带后缀也不算已安装", () => {
    expect(hasClientFootprint(withSource("defaults;pipe-insecure-dacl=verge-mihomo"))).toBe(false);
    expect(hasClientFootprint(withSource("defaults;ipv6-dual-stack=9097"))).toBe(false);
    expect(hasClientFootprint(withSource("unset-default;pipe-insecure-dacl=a"))).toBe(false);
    expect(hasClientFootprint(withSource("flclash:defaults;pipe-insecure-dacl=a"))).toBe(false);
    expect(hasClientFootprint(withSource("preset-verge;pipe-insecure-dacl=a"))).toBe(false);
  });
});
