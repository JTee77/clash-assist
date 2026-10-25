import { describe, expect, it } from "vitest";
import {
  buildProxyMapFromSlim,
  resolveCurrentProxy,
  resolveCurrentProxyFromSlim,
  type SlimGroupLike,
  type SlimNodeLike,
} from "./currentProxy";
import { IGNORE_PROXY_TYPES, JUNK_NAME_KEYWORDS, type ProxyInfo } from "./types";
import corpus from "../../../tests/fixtures/proxies/cases.json";

/**
 * 双端共享语料（`tests/fixtures/proxies/cases.json`）。
 *
 * 同一份文件也被 Rust 读（`nodes.rs` 的 `fixture_group_table_is_transported_losslessly`
 * 用 `include_str!` 内嵌），那边核对组表搬运无损；这里核对判定结果本身。
 * `expectedCurrent` 是人工声明的期望值，代表「这条订阅到底在用哪个节点」——
 * 当前节点判定的规则只在本仓库这一处（`currentProxy.ts`）有实现。
 *
 * 为什么不走读盘：本项目不装 @types/node，测试里 `import "node:fs"` 过不了 tsc；
 * 静态 JSON import 两端都能拿到同一份字节。
 */
type CorpusCase = {
  name: string;
  note: string;
  expectedCurrent: string | null;
  proxies: Record<string, ProxyInfo>;
};

// JSON 推出来的字面量类型不可用（每条用例的键都不同），按语料契约显式收窄一次。
const cases = (corpus as unknown as { cases: CorpusCase[] }).cases;

/** Rust `nodes.rs` 里 GROUP_TYPES 的字面拼写（判定期望值按它挑组）。 */
const GROUP_TYPES = ["Selector", "URLTest", "Fallback", "LoadBalance"];

/** 与 Rust `is_group_type` 等价：规范拼写 + 全小写兜底。 */
const isGroupType = (t: string) =>
  GROUP_TYPES.includes(t) ||
  GROUP_TYPES.some((g) => g.toLowerCase() === t.toLowerCase());

const isJunkName = (name: string) =>
  name.startsWith("PASS") ||
  name.startsWith("REJECT") ||
  JUNK_NAME_KEYWORDS.some((k) => name.includes(k));

/** 按 Rust 的搬运规则，把原始 /proxies 折成前端实际会收到的两份表。 */
function asSlimPayload(proxies: Record<string, ProxyInfo>): {
  nodes: SlimNodeLike[];
  groups: SlimGroupLike[];
} {
  const nodes: SlimNodeLike[] = [];
  const groups: SlimGroupLike[] = [];
  for (const [name, p] of Object.entries(proxies)) {
    if (isGroupType(p.type)) {
      groups.push({ name, type: p.type, now: p.now ?? null, all: p.all ?? [] });
      continue;
    }
    if (IGNORE_PROXY_TYPES.has(p.type) || isJunkName(name)) continue;
    nodes.push({ name, type: p.type });
  }
  return { nodes, groups };
}

describe("共享语料：resolveCurrentProxy 必须等于 expectedCurrent（原始 /proxies 入口）", () => {
  it("语料至少 15 条（删用例等于让对拍空转）", () => {
    expect(cases.length).toBeGreaterThanOrEqual(15);
  });

  for (const c of cases) {
    it(`${c.name} — ${c.note}`, () => {
      expect(resolveCurrentProxy(c.proxies)).toBe(c.expectedCurrent);
    });
  }
});

describe("共享语料：GUI 主路径（Rust 搬运 → 前端重建字典）必须与原始入口同解", () => {
  for (const c of cases) {
    it(`${c.name} — 重建后仍得 ${String(c.expectedCurrent)}`, () => {
      const slim = asSlimPayload(c.proxies);
      const rebuilt = resolveCurrentProxyFromSlim(slim.nodes, slim.groups);
      expect(rebuilt).toBe(resolveCurrentProxy(c.proxies));
      expect(rebuilt).toBe(c.expectedCurrent);
    });

    it(`${c.name} — 策略组的 now / all 必须原样搬到重建字典里`, () => {
      const slim = asSlimPayload(c.proxies);
      const map = buildProxyMapFromSlim(slim.nodes, slim.groups);
      for (const [name, p] of Object.entries(c.proxies)) {
        if (!isGroupType(p.type)) continue;
        expect(map[name]?.type, `组 ${name} 没进重建字典`).toBe(p.type);
        expect(map[name]?.now).toBe(p.now);
        if (p.all) {
          // 少一条 all 就数不出真实叶、跟不了嵌套链，判定会退化成「未知」。
          expect(map[name]?.all, `组 ${name} 的 all 搬运失真`).toEqual(p.all);
        }
      }
    });
  }
});
