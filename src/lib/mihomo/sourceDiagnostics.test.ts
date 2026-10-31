import { describe, expect, it } from "vitest";
import trustRs from "../../../src-tauri/src/mihomo/trust.rs?raw";
import { zhHans } from "../../i18n/locales/zh-Hans";
import backendLabels from "./backendSourceLabels.fixture.json";
import {
  baseSource,
  hasInsecureDacl,
  insecurePipesOf,
  parseSource,
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

describe("后缀结构化解析（PR52 Ellie must-fix 1）", () => {
  it("路径 / 管道名里出现 pipe-insecure 字样不误报", () => {
    expect(hasInsecureDacl("verge-config:C:/pipe-insecure/config.yaml")).toBe(false);
    expect(hasInsecureDacl("verge-pipe:pipe-insecure-dacl")).toBe(false);
    expect(hasInsecureDacl("verge-tcp(listener-verified):C:/pipe-insecure/config.yaml")).toBe(false);
    expect(hasInsecureDacl("verge-tcp(listener-verified,pipe-insecure-x):C:/v/config.yaml")).toBe(false);
    expect(hasInsecureDacl("verge-pipe:b;pipe-open-to-all=pipe-insecure")).toBe(false);
    expect(hasInsecureDacl("verge-pipe:b;x-pipe-insecure-dacl=a")).toBe(false);
    expect(sourceNoticesOf(["verge-config:C:/pipe-insecure/config.yaml"])).toEqual([]);
  });

  it("TCP 括号内只有整项 pipe-insecure 才算", () => {
    expect(hasInsecureDacl("verge-tcp(pipe-insecure,listener-unverified):x")).toBe(true);
    expect(hasInsecureDacl("verge-tcp( listener-verified , pipe-insecure ):x")).toBe(true);
  });

  it("未知键与新键（ipv6-dual-stack / pipe-open-to-all）忽略，不当成管道名", () => {
    const s = "verge-pipe:b;ipv6-dual-stack=9097;pipe-insecure-dacl=a|c;pipe-open-to-all=b;future=1";
    expect(baseSource(s)).toBe("verge-pipe:b");
    expect(insecurePipesOf(s)).toEqual(["a", "c"]);
    expect(sourceNoticesOf([s])).toEqual([{ kind: "insecure-dacl", pipes: ["a", "c"] }]);
    const noDacl = "verge-config:C:/v/config.yaml;ipv6-dual-stack=9097;pipe-open-to-all=x|y;zzz";
    expect(hasInsecureDacl(noDacl)).toBe(false);
    expect(insecurePipesOf(noDacl)).toEqual([]);
    expect(sourceNoticesOf([noDacl])).toEqual([]);
  });

  it("值按百分号解码", () => {
    const s = "verge-pipe:b;pipe-insecure-dacl=verge%3Bmihomo|a%7Cb|c%2Cd%29%3De%25";
    expect(insecurePipesOf(s)).toEqual(["verge;mihomo", "a|b", "c,d)=e%"]);
    expect(parseSource(s)).toEqual({
      base: "verge-pipe:b",
      tcpItems: null,
      insecureDacl: true,
      insecurePipes: ["verge;mihomo", "a|b", "c,d)=e%"],
    });
  });

  it("值解析失败（坏编码 / 未编码保留字符 / 空值 / 缺 =）仍显示通用 InsecureDacl 提示", () => {
    for (const s of [
      "verge-pipe:b;pipe-insecure-dacl=%E4%B8",
      "verge-pipe:b;pipe-insecure-dacl=a|%zz",
      "verge-pipe:b;pipe-insecure-dacl=a=b",
      "verge-pipe:b;pipe-insecure-dacl=a,b",
      "verge-pipe:b;pipe-insecure-dacl=a||b",
      "verge-pipe:b;pipe-insecure-dacl=",
      "verge-pipe:b;pipe-insecure-dacl",
    ]) {
      expect(hasInsecureDacl(s), s).toBe(true);
      expect(insecurePipesOf(s), s).toEqual([]);
      expect(sourceNoticesOf([s]), s).toEqual([{ kind: "insecure-dacl", pipes: [] }]);
      expect(baseSource(s), s).toBe("verge-pipe:b");
    }
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


describe("百分号解码与 Rust 测试解码器一致", () => {
  it("%+1 等非十六进制转义整体解析失败（Rust pct_decode 同样拒绝）", () => {
    for (const bad of ["%+1", "%-1", "%1", "%", "a%g0", "%E4%B8"]) {
      expect(insecurePipesOf(`verge-pipe:b;pipe-insecure-dacl=${bad}`)).toEqual([]);
      expect(hasInsecureDacl(`verge-pipe:b;pipe-insecure-dacl=${bad}`)).toBe(true);
    }
    expect(insecurePipesOf("verge-pipe:b;pipe-insecure-dacl=a%2cb%2C")).toEqual(["a,b,"]);
  });
});

describe("后端生成的来源标签样本（Rust backend_labels_match_frontend_fixture 逐字校验同一份样本）", () => {
  for (const s of backendLabels.labels) {
    it(s.name, () => {
      const p = parseSource(s.label);
      expect(p.base).toBe(s.base);
      expect(p.tcpItems).toEqual(s.tcpItems);
      expect(p.insecureDacl).toBe(s.insecureDacl);
      expect(p.insecurePipes).toEqual(s.insecurePipes);
      expect(hasInsecureDacl(s.label)).toBe(
        s.insecureDacl || (s.tcpItems?.includes("pipe-insecure") ?? false),
      );
      expect(sourceNoticesOf([s.label])).toEqual(s.notices);
      // 路径里编码过的 `;` 不会截断 base，也不会被当成后缀
      expect(baseSource(s.label)).toBe(s.base);
    });
  }

  it("超长标签截断后仍有 pipe-insecure-dacl，且不超过后端上限 1024 字节", () => {
    const s = backendLabels.labels.find((x) => x.name === "truncated-huge-file-list")!;
    expect(new TextEncoder().encode(s.label).length).toBeLessThanOrEqual(1024);
    expect(s.base.endsWith(",%28truncated%29")).toBe(true);
    expect(hasInsecureDacl(s.label)).toBe(true);
    expect(insecurePipesOf(s.label)).toEqual(["\\\\.\\pipe\\verge-mihomo"]);
  });

  it("新标记的值不会被当成管道名，service-mode 不影响 listener-unverified 识别", () => {
    const all = sourceNoticesOf(backendLabels.labels.map((s) => s.label));
    const insecure = all.find((n) => n.kind === "insecure-dacl");
    expect(insecure && "pipes" in insecure ? insecure.pipes : []).toEqual([
      "\\\\.\\pipe\\verge-mihomo",
      "\\\\.\\pipe\\a;b",
    ]);
    expect(all.some((n) => n.kind === "tcp-unverified")).toBe(true);
  });
});
