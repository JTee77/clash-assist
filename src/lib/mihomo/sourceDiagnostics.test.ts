import { describe, expect, it } from "vitest";
import trustRs from "../../../src-tauri/src/mihomo/trust.rs?raw";
import { zhHans } from "../../i18n/locales/zh-Hans";
import {
  baseSource,
  hasInsecureDacl,
  insecurePipesOf,
  sourceNoticesOf,
} from "./sourceDiagnostics";

describe("InsecureDacl 来源识别", () => {
  it("后缀 ;pipe-insecure-dacl= 与 TCP 标签里的 pipe-insecure 都算", () => {
    expect(hasInsecureDacl("verge-pipe:b;pipe-insecure-dacl=a|c")).toBe(true);
    expect(hasInsecureDacl("defaults;pipe-insecure-dacl=verge-mihomo")).toBe(true);
    expect(hasInsecureDacl("verge-tcp(listener-verified,pipe-insecure):C:/v/config.yaml")).toBe(true);
    expect(hasInsecureDacl("verge-pipe:verge-mihomo")).toBe(false);
    expect(hasInsecureDacl("verge-tcp(listener-verified):C:/v/config.yaml")).toBe(false);
    expect(hasInsecureDacl(undefined)).toBe(false);
  });

  it("baseSource / insecurePipesOf 拆分后缀", () => {
    const s = "verge-tcp(listener-verified,pipe-insecure):C:/v/config.yaml;pipe-insecure-dacl=a";
    expect(baseSource(s)).toBe("verge-tcp(listener-verified,pipe-insecure):C:/v/config.yaml");
    expect(insecurePipesOf(s)).toEqual(["a"]);
    expect(insecurePipesOf("verge-pipe:b;pipe-insecure-dacl=a|c")).toEqual(["a", "c"]);
    expect(baseSource("verge-pipe:b")).toBe("verge-pipe:b");
    expect(insecurePipesOf("verge-pipe:b")).toEqual([]);
  });
});

describe("sourceNoticesOf", () => {
  it("选中了别的可信管道时仍提示 InsecureDacl（Ellie PR50 Q5）", () => {
    expect(sourceNoticesOf(["verge-pipe:b;pipe-insecure-dacl=a|c"])).toEqual([
      { kind: "insecure-dacl", pipes: ["a", "c"] },
    ]);
  });

  it("多个候选来源合并、去重，InsecureDacl 排最前", () => {
    expect(
      sourceNoticesOf([
        "verge-tcp(rejected:listener-conflict):C:/v/config.yaml",
        undefined,
        "defaults;pipe-insecure-dacl=a",
        "verge-tcp(listener-unverified,pipe-insecure):C:/v/config.yaml;pipe-insecure-dacl=a|b",
      ]),
    ).toEqual([
      { kind: "insecure-dacl", pipes: ["a", "b"] },
      { kind: "tcp-unverified" },
      { kind: "tcp-rejected", code: "listener-conflict" },
    ]);
  });

  it("不可信管道（M-3）与 TCP 拒绝原因", () => {
    expect(sourceNoticesOf(["verge-pipe-untrusted:C:/v/config.yaml"])).toEqual([
      { kind: "untrusted-pipe" },
    ]);
    expect(
      sourceNoticesOf([
        "verge-tcp(rejected:token-mismatch):x",
        "verge-tcp(rejected:image-mismatch):x",
        "verge-tcp(rejected:token-mismatch):y",
      ]),
    ).toEqual([
      { kind: "tcp-rejected", code: "token-mismatch" },
      { kind: "tcp-rejected", code: "image-mismatch" },
    ]);
  });

  it("正常来源与 macOS 来源不出提示", () => {
    expect(
      sourceNoticesOf([
        "verge-pipe:verge-mihomo",
        "verge-tcp(listener-verified):C:/v/config.yaml",
        "verge-config:C:/v/config.yaml",
        "verge-ec-blank:C:/v/config.yaml",
        "defaults",
        "preset-verge",
        "verge-sock:/tmp/verge/verge-mihomo.sock",
        "",
      ]),
    ).toEqual([]);
  });
});

describe("InsecureDacl 文案与 Rust 常量逐字一致（Ellie 复核 §4）", () => {
  const rustConst = (name: string) =>
    trustRs.match(new RegExp(`pub const ${name}: &str =\\s*"([^"]+)"`))?.[1];

  it("提示与按钮文案", () => {
    expect(rustConst("PIPE_INSECURE_DACL_UI")).toBe(
      "已拒绝连接：该 Verge 内核的控制管道允许本机其他用户冒充，存在被劫持风险。",
    );
    expect(rustConst("PIPE_INSECURE_DACL_ACTION")).toBe("升级 Clash Verge 并重启内核");
    expect(zhHans["diag.insecureDacl"]).toBe(rustConst("PIPE_INSECURE_DACL_UI"));
    expect(zhHans["diag.insecureDaclAction"]).toBe(rustConst("PIPE_INSECURE_DACL_ACTION"));
  });
});
