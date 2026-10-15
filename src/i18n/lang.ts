export type Lang = "zh-Hans" | "zh-Hant" | "en";
export type LangPref = "system" | Lang;

export const LANG_STORAGE_KEY = "clash-assist.lang";

/** 系统语言标签 → 界面语言：zh-Hant / zh-TW / zh-HK / zh-MO → 繁體；其它 zh* → 简体；其余 → English。 */
export function langFromTag(tag: string | null | undefined): Lang {
  const l = (tag ?? "").toLowerCase().replace(/_/g, "-");
  if (!(l === "zh" || l.startsWith("zh-"))) return "en";
  const parts = l.split("-");
  if (parts.includes("hant")) return "zh-Hant";
  if (parts.includes("hans")) return "zh-Hans";
  if (parts.some((p) => p === "tw" || p === "hk" || p === "mo")) return "zh-Hant";
  return "zh-Hans";
}

export function readStoredLangPref(): LangPref {
  try {
    const v = localStorage.getItem(LANG_STORAGE_KEY);
    if (v === "system" || v === "zh-Hans" || v === "zh-Hant" || v === "en") return v;
  } catch {}
  return "system";
}

export function navigatorTag(): string {
  if (typeof navigator === "undefined") return "zh-Hans";
  return navigator.languages?.[0] ?? navigator.language ?? "zh-Hans";
}
