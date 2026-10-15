/**
 * 运行时文案翻译：src/lib 与 Rust 产出的中文（CLI 共用，源头不改）在 GUI 渲染时转换。
 * - zh-Hant：OpenCC（cn → twp，台湾用语）按需懒加载 + 少量术语修正
 * - en：EN_RULES 整句模板（{n} 捕获递归翻译）→ 失败时按行 / 子串兜底
 */
import type { Lang } from "./lang";
import { EN_FRAGMENTS, EN_RULES } from "./enRules";

type Compiled = { re: RegExp; en: string; literal: number };

const HAN = /\p{Script=Han}/u;
const exact = new Map<string, string>();
const templates: Compiled[] = [];

for (const [zh, en] of EN_RULES) {
  if (!/\{\d+\}/.test(zh)) {
    exact.set(zh, en);
    continue;
  }
  const order: number[] = [];
  const src = zh
    .split(/(\{\d+\})/)
    .map((part) => {
      const m = /^\{(\d+)\}$/.exec(part);
      if (m) {
        order.push(Number(m[1]));
        return "([\\s\\S]+?)";
      }
      return part.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
    })
    .join("");
  const literal = zh.replace(/\{\d+\}/g, "").length;
  templates.push({ re: new RegExp(`^${src}$`, "u"), en: en.replace(/\{(\d+)\}/g, (_, n) => `{#${order.indexOf(Number(n))}}`), literal });
}
templates.sort((a, b) => b.literal - a.literal);
const fragments = [...EN_FRAGMENTS].sort((a, b) => b[0].length - a[0].length);

function punct(s: string): string {
  return s
    .replace(/：/g, ": ")
    .replace(/，/g, ", ")
    .replace(/；/g, "; ")
    .replace(/。/g, ". ")
    .replace(/、/g, ", ")
    .replace(/（/g, " (")
    .replace(/）/g, ")")
    .replace(/[「『]/g, "“")
    .replace(/[」』]/g, "”")
    .replace(/[【]/g, "“")
    .replace(/[】]/g, "”")
    .replace(/ {2,}/g, " ")
    .trim();
}

const enCache = new Map<string, string>();

function enLine(line: string, depth: number): string {
  if (!HAN.test(line)) return line;
  const hit = exact.get(line) ?? exact.get(line.trim());
  if (hit !== undefined) return hit;
  if (depth < 4) {
    for (const t of templates) {
      const m = t.re.exec(line);
      if (!m) continue;
      return t.en.replace(/\{#(\d+)\}/g, (_, i) => enLine(m[Number(i) + 1] ?? "", depth + 1));
    }
  }
  // 兜底：子串替换常见短语 + 全角标点
  let out = line;
  for (const [zh, en] of fragments) out = out.split(zh).join(en);
  return punct(out);
}

export function toEnglish(text: string): string {
  if (!HAN.test(text)) return text;
  const c = enCache.get(text);
  if (c !== undefined) return c;
  const leading = text.match(/^\s*/)?.[0] ?? "";
  const out = text
    .split("\n")
    .map((l) => {
      const ind = /^\s*/.exec(l)?.[0] ?? "";
      const tr = enLine(l, 0);
      return tr.startsWith(ind) ? tr : ind + tr;
    })
    .join("\n");
  const res = leading && !out.startsWith(leading) ? leading + out : out;
  enCache.set(text, res);
  return res;
}

// ── 繁體 ──
type Conv = (s: string) => string;
let hantConv: Conv | null = null;
let hantLoading: Promise<void> | null = null;
const HANT_FIXES: [RegExp, string][] = [
  [/高階/g, "進階"],
  [/質量/g, "品質"],
  [/運營商/g, "電信業者"],
  [/獲取/g, "取得"],
  [/演示/g, "展示"],
  [/當前/g, "目前"],
  [/套接字/g, "通訊端"],
  [/默認/g, "預設"],
];

export function loadHant(): Promise<void> {
  if (hantConv) return Promise.resolve();
  if (!hantLoading) {
    hantLoading = import("opencc-js/cn2t").then((m) => {
      const conv = m.Converter({ from: "cn", to: "twp" });
      hantConv = (s) => HANT_FIXES.reduce((acc, [re, to]) => acc.replace(re, to), conv(s));
    });
  }
  return hantLoading;
}

export function hantReady(): boolean {
  return hantConv !== null;
}

export function toHant(text: string): string {
  if (!HAN.test(text)) return text;
  return hantConv ? hantConv(text) : text;
}

export function translateDynamic(text: string | null | undefined, lang: Lang): string {
  if (text == null) return "";
  if (lang === "en") return toEnglish(text);
  if (lang === "zh-Hant") return toHant(text);
  return text;
}
