/** Region grouping — multi-signal detection with static rule table & precompiled regex cache */

import regionRulesData from "./regionRules.json";

export type RegionRule = [string, string[]];

export const REGION_RULES: RegionRule[] = regionRulesData.rules as RegionRule[];

const SHORT_REGION_NAMES: Record<string, string> =
  regionRulesData.shortRegionNames;

/** 紧凑地区标签（供筛选胶囊/下拉展示，内部 region 值保持不变）。 */
export function shortRegionName(region: string): string {
  return SHORT_REGION_NAMES[region] ?? region;
}

/** 国内入口+落地双字专线词（优先级低于明确落地国/城市/旗帜/代号）。 */
const TRANSIT_LINE_PATTERNS = new Set<string>(
  regionRulesData.transitLinePatterns,
);

/** 单字简称映射（仅在独立查询或「港01 / 日-02」等非汉字粘连编号时启用）。 */
const SINGLE_CJK_MAP: Record<string, string> = regionRulesData.singleCjkMap;

/** 易与英文介词/代词撞车的两字母代码：无数字后缀时要求全大写。 */
const CASE_SENSITIVE_BARE_CODES = new Set<string>(
  regionRulesData.caseSensitiveBareCodes,
);

/** 流媒体/AI 解锁与中转/入口描述关键词：所在备注块或修饰子句降权处理。 */
const UNLOCK_OR_TRANSIT_RE =
  /解锁|解鎖|奈飞|奈飛|网飞|網飛|netflix|\bnf\b|disney|迪士尼|tiktok|\btk\b|youtube|\bytb\b|油管|openai|chatgpt|\bgpt\b|claude|gemini|copilot|巴哈|动画疯|動畫瘋|bilibili|b站|哔哩|tvb|dazn|hbo|hulu|prime|spotify|abema|\bdmm\b|流媒体|流媒體|送中|中转|中轉|入口|经由|經由|前置|转发|轉發|\brelay\b|\btransit\b|\bentry\b|\bvia\b/i;

/** 机场冠名 / 套餐线路常见修饰词：当与其他干净落地段冲突时降权。 */
const BRAND_FLAVOR_RE =
  /高速|机场|機場|订阅|訂閱|套餐|专线|專線|网络|網絡|加速|云|雲|频道|頻道|节点|節點|群组|群組|国际|國際|全球|互联|互聯/;

const ARROW_SPLIT_RE = /\s*(?:--+>|==+>|[→⇒➔➜➡])\s*/;

const SINGLE_NUM_PATTERN =
  "(^|[^A-Za-z\\u4e00-\\u9fff])([港台臺日新美韩韓英德法荷澳加俄印马馬泰菲土])\\s*[-_]?\\s*\\d+";

function escapeRegExp(s: string): string {
  return s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

function isFlagEmoji(p: string): boolean {
  return /[\u{1F1E6}-\u{1F1FF}]/u.test(p);
}

function isAreaCompound(p: string): boolean {
  return /^[港台臺日新美韩韓英德法澳加俄印马馬泰菲土阿][区區]$/.test(p);
}

type CompiledPattern =
  | {
      kind: "literal";
      region: string;
      pattern: string;
      tier: number;
    }
  | {
      kind: "asciiWord";
      region: string;
      re: RegExp;
    }
  | {
      kind: "code";
      region: string;
      withDigitsRe: RegExp;
      bareRe: RegExp;
      requireUpperWithDigits: boolean;
    };

/** 模块初始化时一次性预编译全部 500+ 词条，运行时零 RegExp 动态分配。 */
const COMPILED_PATTERNS: readonly CompiledPattern[] = (() => {
  const list: CompiledPattern[] = [];
  for (const [region, patterns] of REGION_RULES) {
    for (const p of patterns) {
      if (isFlagEmoji(p)) {
        list.push({ kind: "literal", region, pattern: p, tier: 4 });
        continue;
      }

      if (/[\u4e00-\u9fff]/.test(p)) {
        const tier = TRANSIT_LINE_PATTERNS.has(p)
          ? 1
          : isAreaCompound(p)
            ? 2
            : 3;
        list.push({ kind: "literal", region, pattern: p, tier });
        continue;
      }

      const escaped = escapeRegExp(p);
      if (p.length >= 4) {
        list.push({
          kind: "asciiWord",
          region,
          re: new RegExp(`(^|[^A-Za-z])(${escaped})(?=[^A-Za-z]|$)`, "i"),
        });
        continue;
      }

      const caseSensitive = CASE_SENSITIVE_BARE_CODES.has(p);
      list.push({
        kind: "code",
        region,
        withDigitsRe: new RegExp(
          `(^|[^A-Za-z0-9])(${escaped})\\d+(?![A-Za-z])`,
          "i",
        ),
        bareRe: new RegExp(
          `(^|[^A-Za-z0-9])(${escaped})(?=[^A-Za-z0-9]|$)`,
          caseSensitive ? "" : "i",
        ),
        requireUpperWithDigits: caseSensitive && p !== "US",
      });
    }
  }
  return list;
})();

interface RegionMatch {
  region: string;
  tier: number; // 4: flag, 3: country/city/code, 2: area/single+num, 1: transit compound
  index: number;
}

/** 在单个片段内查找匹配，返回片段中最高优先级（同级取靠后位置）的地区匹配。 */
function matchSegment(seg: string): RegionMatch | null {
  let best: RegionMatch | null = null;

  const consider = (region: string, tier: number, index: number) => {
    if (
      !best ||
      tier > best.tier ||
      (tier === best.tier && index > best.index)
    ) {
      best = { region, tier, index };
    }
  };

  for (const item of COMPILED_PATTERNS) {
    if (item.kind === "literal") {
      const idx = seg.indexOf(item.pattern);
      if (idx !== -1) consider(item.region, item.tier, idx);
      continue;
    }

    if (item.kind === "asciiWord") {
      const m = item.re.exec(seg);
      if (m) {
        const idx = m.index + (m[1]?.length ?? 0);
        consider(item.region, 3, idx);
      }
      continue;
    }

    // 2~3 字母国家码 / 机房三字码：
    // 1) 后跟数字编号（如 JP1、JP80、US01、HK22），左边界必须非字母数字（防 Node1/Plus1/Main1）
    const md = item.withDigitsRe.exec(seg);
    if (md) {
      const matchedCode = md[2] ?? "";
      if (
        !item.requireUpperWithDigits ||
        matchedCode === matchedCode.toUpperCase()
      ) {
        const idx = md.index + (md[1]?.length ?? 0);
        consider(item.region, 3, idx);
        continue;
      }
    }

    // 2) 独立代码（如 HK、JP、US-01、LAX-01）
    const mb = item.bareRe.exec(seg);
    if (mb) {
      const idx = mb.index + (mb[1]?.length ?? 0);
      consider(item.region, 3, idx);
    }
  }

  // 单字中文 + 编号（如 港01、日-02、美 01），要求左右无其他汉字粘连
  const singleNumRe = new RegExp(SINGLE_NUM_PATTERN, "g");
  let sm: RegExpExecArray | null;
  while ((sm = singleNumRe.exec(seg)) !== null) {
    const ch = sm[2];
    const region = ch ? SINGLE_CJK_MAP[ch] : undefined;
    if (region) {
      const idx = sm.index + (sm[1]?.length ?? 0);
      consider(region, 2, idx);
    }
  }

  return best;
}

/**
 * 拆分节点名称为「主干片段列表」与「备注片段列表（流媒体解锁 / 中转入口说明）」。
 */
function splitPrimaryAndRemarks(input: string): {
  primarySegments: string[];
  remarkSegments: string[];
} {
  const remarkSegments: string[] = [];

  // 1. 先提取含解锁/中转关键词的括号块
  const strippedBrackets = input.replace(
    /[(\uff08[\u3010\u300c]([^()\uff08\uff09[\]\u3010\u3011\u300c\u300d]+)[)\uff09\]\u3011\u300d]/g,
    (_full, inner: string) => {
      if (UNLOCK_OR_TRANSIT_RE.test(inner)) {
        remarkSegments.push(inner);
        return " | ";
      }
      return ` | ${inner} | `;
    },
  );

  // 2. 按结构分隔符切分片段
  const rawSegments = strippedBrackets
    .split(/[-—–_|/·:]+/)
    .map((s) => s.trim())
    .filter(Boolean);

  const primarySegments: string[] = [];

  for (const seg of rawSegments) {
    const tokens = seg.split(/\s+/).filter(Boolean);
    if (tokens.length === 0) continue;

    // 若某个分段本身含解锁/中转词，且分段内没有任何带数字编号的节点主干词（例如 "| 解锁美区 Netflix"、"| 美区 Netflix"、"- 香港中转"），整段归为备注
    if (UNLOCK_OR_TRANSIT_RE.test(seg)) {
      const keptTokens: string[] = [];
      for (const tok of tokens) {
        if (UNLOCK_OR_TRANSIT_RE.test(tok)) {
          remarkSegments.push(tok);
        } else if (!/\d/.test(tok) && tokens.length > 1) {
          // 同段内无数字的修饰词（如 "美区 Netflix" 中的 "美区"、"香港 中转" 中的 "香港"）随备注一起剥离
          remarkSegments.push(tok);
        } else {
          keptTokens.push(tok);
        }
      }
      if (keptTokens.length > 0) {
        primarySegments.push(keptTokens.join(" "));
      }
    } else {
      primarySegments.push(seg);
    }
  }

  return { primarySegments, remarkSegments };
}

/** 只在主干片段（不含解锁/中转备注）上识别地区；未命中返回 null。 */
function detectPrimaryRegion(text: string): string | null {
  const trimmed = text.trim();
  if (!trimmed) return null;

  // 方向箭头（如 香港 -> 美国 01）：优先取箭头右侧终点
  if (ARROW_SPLIT_RE.test(trimmed)) {
    const parts = trimmed
      .split(ARROW_SPLIT_RE)
      .map((s) => s.trim())
      .filter(Boolean);
    if (parts.length >= 2) {
      const dest = detectPrimaryRegion(parts[parts.length - 1]);
      if (dest) return dest;
    }
  }

  const { primarySegments } = splitPrimaryAndRemarks(trimmed);
  if (primarySegments.length === 0) return null;

  interface ScoredSegment {
    segIndex: number;
    region: string;
    tier: number;
    matchIndex: number;
    hasBrandFlavor: boolean;
  }

  const hits: ScoredSegment[] = [];
  for (let i = 0; i < primarySegments.length; i++) {
    const seg = primarySegments[i];
    const m = matchSegment(seg);
    if (m) {
      hits.push({
        segIndex: i,
        region: m.region,
        tier: m.tier,
        matchIndex: m.index,
        hasBrandFlavor: BRAND_FLAVOR_RE.test(seg),
      });
    }
  }

  if (hits.length === 0) return null;
  if (hits.length === 1) return hits[0].region;

  // 多段命中不同地区时的裁决顺序：
  // 1) 明确地区（tier >= 2）优先于国内双字专线词（tier === 1，如 深港/沪日）
  // 2) 不带「高速/机场/专线/云」等冠名修饰的干净节点段，优先于带冠名修饰的前缀/后缀段（如 香港高速3-美国1）
  // 3) 更高信号等级（如含国旗 Emoji）优先，或结构上靠后的落地段优先于靠前的前缀段
  let winner = hits[0];
  for (let i = 1; i < hits.length; i++) {
    const cur = hits[i];
    if (cur.region === winner.region) {
      winner = cur;
      continue;
    }
    const winnerIsTransitLine = winner.tier === 1;
    const curIsTransitLine = cur.tier === 1;
    if (winnerIsTransitLine !== curIsTransitLine) {
      if (!curIsTransitLine) winner = cur;
      continue;
    }
    if (winner.hasBrandFlavor !== cur.hasBrandFlavor) {
      if (!cur.hasBrandFlavor) winner = cur;
      continue;
    }
    // 同等性质分段：靠后的主干落地段优先（如 [香港前缀] - [日本01]）
    winner = cur;
  }

  return winner.region;
}

export function detectRegion(name: string): string {
  const trimmed = name.trim();
  if (!trimmed) return "其他地区";

  // 单字独立查询（如 CLI --region 港 / 美 / 日）
  if (trimmed.length === 1 && SINGLE_CJK_MAP[trimmed]) {
    return SINGLE_CJK_MAP[trimmed];
  }

  // 1. 主干识别（已剔除流媒体解锁与中转备注）
  const primary = detectPrimaryRegion(trimmed);
  if (primary) return primary;

  // 2. 若主干不含任何地区（例如节点仅命名为「美区奈飞解锁 01」），回退扫描备注段
  const { remarkSegments } = splitPrimaryAndRemarks(trimmed);
  for (const rem of remarkSegments) {
    const m = matchSegment(rem);
    if (m) return m.region;
  }

  // 3. 完整原串兜底扫描
  const fullMatch = matchSegment(trimmed);
  if (fullMatch) return fullMatch.region;

  return "其他地区";
}

/**
 * 提取节点开头的候选公共前缀（含分隔符，如 "香港高速3-"、"[某某云] "）。
 */
function extractCandidatePrefix(
  name: string,
): { prefix: string; rest: string } | null {
  const m =
    /^(?:\[[^\]]+\]|【[^】]+】|[^-—–_|/·:\s]{2,16})\s*[-—–_|/·:]\s*/.exec(
      name.trim(),
    );
  if (!m) return null;
  const prefix = m[0];
  const rest = name.trim().slice(prefix.length).trim();
  if (!rest) return null;
  return { prefix, rest };
}

/**
 * 批量识别节点列表的地区：
 * 自动检测跨国机场的公共冠名前缀（仅当同一前缀后方的节点主干出现 >= 2 个不同国家，或出现与前缀自身不同的明确国家时才剥离），
 * 不会误伤纯单一地区的专线套餐（如「香港BGP-01..04」）。
 */
export function detectRegions(names: readonly string[]): string[] {
  if (names.length === 0) return [];

  const prefixGroups = new Map<string, { indices: number[]; rests: string[] }>();
  for (let i = 0; i < names.length; i++) {
    const split = extractCandidatePrefix(names[i]);
    if (!split) continue;
    const key = split.prefix;
    const group = prefixGroups.get(key) ?? { indices: [], rests: [] };
    group.indices.push(i);
    group.rests.push(split.rest);
    prefixGroups.set(key, group);
  }

  const confirmedCrossRegionPrefixes = new Set<string>();
  for (const [prefix, group] of prefixGroups.entries()) {
    if (group.indices.length < 2) continue;
    const prefixRegion = detectPrimaryRegion(prefix);
    const restRegions = new Set<string>();
    for (const rest of group.rests) {
      const r = detectPrimaryRegion(rest);
      if (r) restRegions.add(r);
    }
    // 只有当该前缀后面的节点主干明确出现了 >= 2 个不同国家，或者出现了与前缀不同的国家时，才认定为跨国机场前缀
    if (
      restRegions.size >= 2 ||
      (prefixRegion && restRegions.size === 1 && !restRegions.has(prefixRegion))
    ) {
      confirmedCrossRegionPrefixes.add(prefix);
    }
  }

  return names.map((name) => {
    const split = extractCandidatePrefix(name);
    if (split && confirmedCrossRegionPrefixes.has(split.prefix)) {
      const restRegion = detectRegion(split.rest);
      if (restRegion !== "其他地区") return restRegion;
    }
    return detectRegion(name);
  });
}

export function classifyByRegion<T extends { name: string }>(
  nodes: T[],
): [string, T[]][] {
  const regions = detectRegions(nodes.map((n) => n.name));
  const grouped = new Map<string, T[]>();
  for (let i = 0; i < nodes.length; i++) {
    const n = nodes[i];
    const reg = regions[i];
    const list = grouped.get(reg) ?? [];
    list.push(n);
    grouped.set(reg, list);
  }
  return [...grouped.entries()].sort((a, b) => b[1].length - a[1].length);
}
