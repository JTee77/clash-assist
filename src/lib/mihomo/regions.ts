/** Region grouping — multi-signal detection with remark filtering & shared-prefix awareness */

export type RegionRule = [string, string[]];

export const REGION_RULES: RegionRule[] = [
  [
    "中国香港",
    [
      "中国香港",
      "中國香港",
      "香港",
      "港区",
      "港區",
      "深港",
      "广港",
      "廣港",
      "莞港",
      "沪港",
      "滬港",
      "京港",
      "Hong Kong",
      "HongKong",
      "HKG",
      "HK",
      "🇭🇰",
    ],
  ],
  [
    "中国台湾",
    [
      "中国台湾",
      "中國臺灣",
      "中國台灣",
      "台湾",
      "台灣",
      "臺湾",
      "臺灣",
      "台区",
      "台區",
      "臺區",
      "台北",
      "臺北",
      "新北",
      "高雄",
      "台中",
      "臺中",
      "桃园",
      "桃園",
      "彰化",
      "深台",
      "闽台",
      "閩台",
      "沪台",
      "滬台",
      "Taiwan",
      "Taipei",
      "Taichung",
      "Kaohsiung",
      "TWN",
      "TPE",
      "KHH",
      "TW",
      "🇹🇼",
    ],
  ],
  [
    "日本",
    [
      "日本",
      "日区",
      "日區",
      "东京",
      "東京",
      "大阪",
      "名古屋",
      "埼玉",
      "横滨",
      "橫濱",
      "福冈",
      "福岡",
      "札幌",
      "千叶",
      "千葉",
      "沪日",
      "滬日",
      "苏日",
      "蘇日",
      "广日",
      "廣日",
      "深日",
      "京日",
      "Japan",
      "Tokyo",
      "Osaka",
      "Nagoya",
      "Saitama",
      "Fukuoka",
      "Sapporo",
      "JPN",
      "NRT",
      "HND",
      "KIX",
      "TYO",
      "OSA",
      "JP",
      "🇯🇵",
    ],
  ],
  [
    "新加坡",
    [
      "新加坡",
      "狮城",
      "獅城",
      "新区",
      "新區",
      "深新",
      "广新",
      "廣新",
      "沪新",
      "滬新",
      "京新",
      "Singapore",
      "SGP",
      "SIN",
      "SG",
      "🇸🇬",
    ],
  ],
  [
    "美国",
    [
      "美国",
      "美國",
      "美区",
      "美區",
      "美西",
      "美东",
      "美東",
      "美中",
      "洛杉矶",
      "洛杉磯",
      "硅谷",
      "矽谷",
      "圣何塞",
      "聖荷西",
      "圣荷西",
      "纽约",
      "紐約",
      "西雅图",
      "西雅圖",
      "芝加哥",
      "凤凰城",
      "鳳凰城",
      "达拉斯",
      "達拉斯",
      "波特兰",
      "波特蘭",
      "俄勒冈",
      "俄勒岡",
      "亚特兰大",
      "亞特蘭大",
      "迈阿密",
      "邁阿密",
      "丹佛",
      "休斯顿",
      "休斯頓",
      "休士顿",
      "休士頓",
      "盐湖城",
      "鹽湖城",
      "阿什本",
      "弗吉尼亚",
      "維吉尼亞",
      "拉斯维加斯",
      "拉斯維加斯",
      "旧金山",
      "舊金山",
      "三藩",
      "沪美",
      "滬美",
      "广美",
      "廣美",
      "深美",
      "京美",
      "United States",
      "Los Angeles",
      "San Jose",
      "Silicon Valley",
      "Seattle",
      "New York",
      "Chicago",
      "Phoenix",
      "Dallas",
      "Portland",
      "Oregon",
      "Atlanta",
      "Miami",
      "Ashburn",
      "Virginia",
      "Las Vegas",
      "San Francisco",
      "USA",
      "LAX",
      "SJC",
      "SEA",
      "ORD",
      "DFW",
      "JFK",
      "EWR",
      "IAD",
      "MIA",
      "PHX",
      "PDX",
      "DEN",
      "SLC",
      "ATL",
      "SFO",
      "US",
      "🇺🇸",
    ],
  ],
  [
    "韩国",
    [
      "韩国",
      "韓國",
      "韩区",
      "韓區",
      "首尔",
      "首爾",
      "釜山",
      "仁川",
      "春川",
      "沪韩",
      "滬韓",
      "青韩",
      "青韓",
      "威韩",
      "威韓",
      "京韩",
      "京韓",
      "鲁韩",
      "魯韓",
      "South Korea",
      "Korea",
      "Seoul",
      "Busan",
      "Incheon",
      "Chuncheon",
      "KOR",
      "ICN",
      "SEL",
      "PUS",
      "KR",
      "🇰🇷",
    ],
  ],
  [
    "英国",
    [
      "英国",
      "英國",
      "英区",
      "英區",
      "伦敦",
      "倫敦",
      "曼彻斯特",
      "曼徹斯特",
      "伯明翰",
      "爱丁堡",
      "愛丁堡",
      "United Kingdom",
      "Great Britain",
      "England",
      "Britain",
      "London",
      "Manchester",
      "GBR",
      "LHR",
      "LON",
      "UK",
      "GB",
      "🇬🇧",
    ],
  ],
  [
    "德国",
    [
      "德国",
      "德國",
      "德区",
      "德區",
      "法兰克福",
      "法蘭克福",
      "慕尼黑",
      "柏林",
      "杜塞尔多夫",
      "杜塞爾多夫",
      "纽伦堡",
      "紐倫堡",
      "京德",
      "沪德",
      "滬德",
      "Germany",
      "Frankfurt",
      "Munich",
      "Berlin",
      "Dusseldorf",
      "Nuremberg",
      "DEU",
      "FRA",
      "MUC",
      "BER",
      "DE",
      "🇩🇪",
    ],
  ],
  [
    "法国",
    [
      "法国",
      "法國",
      "法区",
      "法區",
      "巴黎",
      "马赛",
      "馬賽",
      "里昂",
      "France",
      "Paris",
      "Marseille",
      "Lyon",
      "CDG",
      "PAR",
      "MRS",
      "FR",
      "🇫🇷",
    ],
  ],
  [
    "荷兰",
    [
      "荷兰",
      "荷蘭",
      "阿姆斯特丹",
      "鹿特丹",
      "Netherlands",
      "Holland",
      "Amsterdam",
      "Rotterdam",
      "NLD",
      "AMS",
      "NL",
      "🇳🇱",
    ],
  ],
  [
    "澳大利亚",
    [
      "澳大利亚",
      "澳大利亞",
      "澳洲",
      "澳区",
      "澳區",
      "悉尼",
      "雪梨",
      "墨尔本",
      "墨爾本",
      "布里斯班",
      "珀斯",
      "伯斯",
      "堪培拉",
      "Australia",
      "Sydney",
      "Melbourne",
      "Brisbane",
      "Perth",
      "AUS",
      "SYD",
      "MEL",
      "BNE",
      "PER",
      "AU",
      "🇦🇺",
    ],
  ],
  [
    "加拿大",
    [
      "加拿大",
      "加区",
      "加區",
      "温哥华",
      "溫哥華",
      "多伦多",
      "多倫多",
      "蒙特利尔",
      "蒙特利爾",
      "满地可",
      "滿地可",
      "Canada",
      "Vancouver",
      "Toronto",
      "Montreal",
      "CAN",
      "YVR",
      "YYZ",
      "YUL",
      "CA",
      "🇨🇦",
    ],
  ],
  ["爱沙尼亚", ["爱沙尼亚", "愛沙尼亞", "塔林", "Estonia", "Tallinn", "EST", "TLL", "EE", "🇪🇪"]],
  [
    "俄罗斯",
    [
      "俄罗斯",
      "俄羅斯",
      "俄区",
      "俄區",
      "莫斯科",
      "圣彼得堡",
      "聖彼得堡",
      "新西伯利亚",
      "新西伯利亞",
      "海参崴",
      "海參崴",
      "符拉迪沃斯托克",
      "伯力",
      "Russia",
      "Moscow",
      "Saint Petersburg",
      "Novosibirsk",
      "Vladivostok",
      "RUS",
      "SVO",
      "DME",
      "VVO",
      "RU",
      "🇷🇺",
    ],
  ],
  [
    "印度",
    [
      "印度",
      "印区",
      "印區",
      "孟买",
      "孟買",
      "新德里",
      "班加罗尔",
      "班加羅爾",
      "金奈",
      "钦奈",
      "India",
      "Mumbai",
      "New Delhi",
      "Bangalore",
      "Chennai",
      "IND",
      "BOM",
      "BLR",
      "MAA",
      "IN",
      "🇮🇳",
    ],
  ],
  [
    "马来西亚",
    [
      "马来西亚",
      "馬來西亞",
      "大马",
      "大馬",
      "马区",
      "馬區",
      "吉隆坡",
      "新山",
      "槟城",
      "檳城",
      "Malaysia",
      "Kuala Lumpur",
      "Johor",
      "Penang",
      "MYS",
      "KUL",
      "MY",
      "🇲🇾",
    ],
  ],
  [
    "泰国",
    [
      "泰国",
      "泰國",
      "泰区",
      "泰區",
      "曼谷",
      "清迈",
      "清邁",
      "普吉",
      "Thailand",
      "Bangkok",
      "Chiang Mai",
      "THA",
      "BKK",
      "TH",
      "🇹🇭",
    ],
  ],
  [
    "菲律宾",
    [
      "菲律宾",
      "菲律賓",
      "菲区",
      "菲區",
      "马尼拉",
      "馬尼拉",
      "宿务",
      "宿霧",
      "Philippines",
      "Manila",
      "Cebu",
      "PHL",
      "MNL",
      "PH",
      "🇵🇭",
    ],
  ],
  [
    "土耳其",
    [
      "土耳其",
      "土区",
      "土區",
      "伊斯坦布尔",
      "伊斯坦布爾",
      "伊斯坦堡",
      "安卡拉",
      "Turkey",
      "Turkiye",
      "Türkiye",
      "Istanbul",
      "Ankara",
      "TUR",
      "IST",
      "TR",
      "🇹🇷",
    ],
  ],
  [
    "阿根廷",
    [
      "阿根廷",
      "阿区",
      "阿區",
      "布宜诺斯艾利斯",
      "布宜諾斯艾利斯",
      "Argentina",
      "Buenos Aires",
      "ARG",
      "EZE",
      "AR",
      "🇦🇷",
    ],
  ],
  [
    "巴西",
    [
      "巴西",
      "圣保罗",
      "聖保羅",
      "里约",
      "里約",
      "Brazil",
      "Sao Paulo",
      "São Paulo",
      "Rio",
      "BRA",
      "GRU",
      "BR",
      "🇧🇷",
    ],
  ],
  ["瑞士", ["瑞士", "苏黎世", "蘇黎世", "日内瓦", "日內瓦", "Switzerland", "Zurich", "Zürich", "Geneva", "CHE", "ZRH", "GVA", "CH", "🇨🇭"]],
  ["瑞典", ["瑞典", "斯德哥尔摩", "斯德哥爾摩", "Sweden", "Stockholm", "SWE", "ARN", "SE", "🇸🇪"]],
];

const SHORT_REGION_NAMES: Record<string, string> = {
  中国香港: "香港",
  中国台湾: "台湾",
  澳大利亚: "澳洲",
  其他地区: "其他",
};

/** 紧凑地区标签（供筛选胶囊/下拉展示，内部 region 值保持不变）。 */
export function shortRegionName(region: string): string {
  return SHORT_REGION_NAMES[region] ?? region;
}

/** 国内入口+落地双字专线词（优先级低于明确落地国/城市/旗帜/代号）。 */
const TRANSIT_LINE_PATTERNS = new Set([
  "深港",
  "广港",
  "廣港",
  "莞港",
  "沪港",
  "滬港",
  "京港",
  "深台",
  "闽台",
  "閩台",
  "沪台",
  "滬台",
  "沪日",
  "滬日",
  "苏日",
  "蘇日",
  "广日",
  "廣日",
  "深日",
  "京日",
  "深新",
  "广新",
  "廣新",
  "沪新",
  "滬新",
  "京新",
  "沪美",
  "滬美",
  "广美",
  "廣美",
  "深美",
  "京美",
  "沪韩",
  "滬韓",
  "青韩",
  "青韓",
  "威韩",
  "威韓",
  "京韩",
  "京韓",
  "鲁韩",
  "魯韓",
  "京德",
  "沪德",
  "滬德",
]);

/** 单字简称映射（仅在独立查询或「港01 / 日-02」等非汉字粘连编号时启用）。 */
const SINGLE_CJK_MAP: Record<string, string> = {
  港: "中国香港",
  台: "中国台湾",
  臺: "中国台湾",
  日: "日本",
  新: "新加坡",
  狮: "新加坡",
  獅: "新加坡",
  美: "美国",
  韩: "韩国",
  韓: "韩国",
  英: "英国",
  德: "德国",
  法: "法国",
  荷: "荷兰",
  澳: "澳大利亚",
  加: "加拿大",
  俄: "俄罗斯",
  印: "印度",
  马: "马来西亚",
  馬: "马来西亚",
  泰: "泰国",
  菲: "菲律宾",
  土: "土耳其",
};

/** 易与英文介词/代词撞车的两字母代码：无数字后缀时要求全大写。 */
const CASE_SENSITIVE_BARE_CODES = new Set(["IN", "MY", "US", "IT", "OR", "AT", "TO", "NO", "SO", "BE", "DO", "GO", "IS", "AS", "BY"]);

/** 流媒体/AI 解锁与中转/入口描述关键词：所在备注块或修饰子句降权处理。 */
const UNLOCK_OR_TRANSIT_RE =
  /解锁|解鎖|奈飞|奈飛|网飞|網飛|netflix|\bnf\b|disney|迪士尼|tiktok|\btk\b|youtube|\bytb\b|油管|openai|chatgpt|\bgpt\b|claude|gemini|copilot|巴哈|动画疯|動畫瘋|bilibili|b站|哔哩|tvb|dazn|hbo|hulu|prime|spotify|abema|\bdmm\b|流媒体|流媒體|送中|中转|中轉|入口|经由|經由|前置|转发|轉發|\brelay\b|\btransit\b|\bentry\b|\bvia\b/i;

/** 机场冠名 / 套餐线路常见修饰词：当与其他干净落地段冲突时降权。 */
const BRAND_FLAVOR_RE =
  /高速|机场|機場|订阅|訂閱|套餐|专线|專線|网络|網絡|加速|云|雲|频道|頻道|节点|節點|群组|群組|国际|國際|全球|互联|互聯/;

const ARROW_SPLIT_RE = /\s*(?:--+>|==+>|[→⇒➔➜➡])\s*/;

function escapeRegExp(s: string): string {
  return s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

function isFlagEmoji(p: string): boolean {
  return /[\u{1F1E6}-\u{1F1FF}]/u.test(p);
}

function isAreaCompound(p: string): boolean {
  return /^[港台臺日新美韩韓英德法澳加俄印马馬泰菲土阿][区區]$/.test(p);
}

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

  for (const [region, patterns] of REGION_RULES) {
    for (const p of patterns) {
      if (isFlagEmoji(p)) {
        const idx = seg.indexOf(p);
        if (idx !== -1) consider(region, 4, idx);
        continue;
      }

      const hasCjk = /[\u4e00-\u9fff]/.test(p);
      if (hasCjk) {
        const idx = seg.indexOf(p);
        if (idx !== -1) {
          const tier = TRANSIT_LINE_PATTERNS.has(p)
            ? 1
            : isAreaCompound(p)
              ? 2
              : 3;
          consider(region, tier, idx);
        }
        continue;
      }

      // ASCII 国家/城市全名（长度 >= 4）
      if (p.length >= 4) {
        const re = new RegExp(`(^|[^A-Za-z])(${escapeRegExp(p)})(?=[^A-Za-z]|$)`, "i");
        const m = re.exec(seg);
        if (m) {
          const idx = m.index + (m[1]?.length ?? 0);
          consider(region, 3, idx);
        }
        continue;
      }

      // 2~3 字母国家码 / 机房三字码：
      // 1) 后跟数字编号（如 JP1、JP80、US01、HK22），左边界必须非字母数字（防 Node1/Plus1/Main1）
      const withDigitsRe = new RegExp(
        `(^|[^A-Za-z0-9])(${escapeRegExp(p)})\\d+(?![A-Za-z])`,
        "i",
      );
      const md = withDigitsRe.exec(seg);
      if (md) {
        const matchedCode = md[2] ?? "";
        // 对 in1 / my1 等极易撞词的小写形式要求大写；常见代码（jp1/us1/hk1/sg1/kr1/tw1/de1/fr1/uk1）大小写皆可
        if (
          !CASE_SENSITIVE_BARE_CODES.has(p) ||
          p === "US" ||
          matchedCode === matchedCode.toUpperCase()
        ) {
          const idx = md.index + (md[1]?.length ?? 0);
          consider(region, 3, idx);
          continue;
        }
      }

      // 2) 独立代码（如 HK、JP、US-01、LAX-01）
      const flags = CASE_SENSITIVE_BARE_CODES.has(p) ? "" : "i";
      const bareRe = new RegExp(
        `(^|[^A-Za-z0-9])(${escapeRegExp(p)})(?=[^A-Za-z0-9]|$)`,
        flags,
      );
      const mb = bareRe.exec(seg);
      if (mb) {
        const idx = mb.index + (mb[1]?.length ?? 0);
        consider(region, 3, idx);
      }
    }
  }

  // 单字中文 + 编号（如 港01、日-02、美 01），要求左右无其他汉字粘连
  const singleNumRe =
    /(^|[^A-Za-z\u4e00-\u9fff])([港台臺日新美韩韓英德法荷澳加俄印马馬泰菲土])\s*[-_]?\s*\d+/g;
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
    const parts = trimmed.split(ARROW_SPLIT_RE).map((s) => s.trim()).filter(Boolean);
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
function extractCandidatePrefix(name: string): { prefix: string; rest: string } | null {
  const m = /^(?:\[[^\]]+\]|【[^】]+】|[^-—–_|/·:\s]{2,16})\s*[-—–_|/·:]\s*/.exec(
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
