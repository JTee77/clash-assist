/** Region grouping — same spirit as reference/clash_speedtest.py REGION_RULES */

export type RegionRule = [string, string[]];

export const REGION_RULES: RegionRule[] = [
  ["中国香港", ["香港", "港", "HK", "HongKong", "Hong Kong", "HKG", "🇭🇰"]],
  ["中国台湾", ["台湾", "臺湾", "台北", "新北", "高雄", "TW", "Taiwan", "TWN", "🇹🇼"]],
  ["日本", ["日本", "东京", "大阪", "TY", "OS", "JP", "Japan", "JPN", "Tokyo", "Osaka", "🇯🇵"]],
  ["新加坡", ["新加坡", "狮城", "SG", "Singapore", "SGP", "🇸🇬"]],
  [
    "美国",
    [
      "美国",
      "美",
      "US",
      "USA",
      "United States",
      "洛杉矶",
      "硅谷",
      "圣何塞",
      "纽约",
      "西雅图",
      "芝加哥",
      "凤凰城",
      "达拉斯",
      "波特兰",
      "俄勒冈",
      "🇺🇸",
    ],
  ],
  ["韩国", ["韩国", "韓", "KR", "Korea", "KOR", "首尔", "首爾", "🇰🇷"]],
  ["英国", ["英国", "英", "UK", "GB", "GBR", "Great Britain", "England", "伦敦", "London", "🇬🇧"]],
  ["德国", ["德国", "德", "DE", "DEU", "Germany", "法兰克福", "Frankfurt", "🇩🇪"]],
  ["法国", ["法国", "法", "FR", "FRA", "France", "巴黎", "Paris", "🇫🇷"]],
  ["荷兰", ["荷兰", "荷", "NL", "NLD", "Netherlands", "阿姆斯特丹", "Amsterdam", "🇳🇱"]],
  [
    "澳大利亚",
    ["澳大利亚", "澳洲", "AU", "AUS", "Australia", "悉尼", "墨尔本", "Sydney", "Melbourne", "🇦🇺"],
  ],
  ["加拿大", ["加拿大", "加", "CA", "CAN", "Canada", "温哥华", "多伦多", "Vancouver", "Toronto", "🇨🇦"]],
  ["爱沙尼亚", ["爱沙尼亚", "EE", "EST", "Estonia", "🇪🇪"]],
  ["俄罗斯", ["俄罗斯", "俄", "RU", "RUS", "Russia", "莫斯科", "Moscow", "🇷🇺"]],
  ["印度", ["印度", "IN", "IND", "India", "孟买", "Mumbai", "🇮🇳"]],
  ["马来西亚", ["马来西亚", "大马", "MY", "MYS", "Malaysia", "吉隆坡", "Kuala Lumpur", "🇲🇾"]],
  ["泰国", ["泰国", "泰", "TH", "THA", "Thailand", "曼谷", "Bangkok", "🇹🇭"]],
  ["菲律宾", ["菲律宾", "PH", "PHL", "Philippines", "马尼拉", "Manila", "🇵🇭"]],
  ["土耳其", ["土耳其", "TR", "TUR", "Turkey", "伊斯坦布尔", "Istanbul", "🇹🇷"]],
  ["阿根廷", ["阿根廷", "AR", "ARG", "Argentina", "🇦🇷"]],
  ["巴西", ["巴西", "BR", "BRA", "Brazil", "🇧🇷"]],
  ["瑞士", ["瑞士", "CH", "CHE", "Switzerland", "苏黎世", "🇨🇭"]],
  ["瑞典", ["瑞典", "SE", "SWE", "Sweden", "斯德哥尔摩", "🇸🇪"]],
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

function escapeRegExp(s: string): string {
  return s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

export function detectRegion(name: string): string {
  for (const [region, patterns] of REGION_RULES) {
    for (const p of patterns) {
      const hasCjk = /[\u4e00-\u9fa5]/.test(p);
      if (hasCjk && name.includes(p)) return region;
      const re = new RegExp(`(^|[^\\w])${escapeRegExp(p)}([^\\w]|$)`, "i");
      if (re.test(name)) return region;
    }
  }
  const m = name.match(/^([\u4e00-\u9fa5]{2,4})/);
  if (m) return m[1];
  return "其他地区";
}

export function classifyByRegion<T extends { name: string }>(
  nodes: T[],
): [string, T[]][] {
  const grouped = new Map<string, T[]>();
  for (const n of nodes) {
    const reg = detectRegion(n.name);
    const list = grouped.get(reg) ?? [];
    list.push(n);
    grouped.set(reg, list);
  }
  return [...grouped.entries()].sort((a, b) => b[1].length - a[1].length);
}
