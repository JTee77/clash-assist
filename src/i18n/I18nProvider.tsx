import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from "react";
import { invoke } from "@tauri-apps/api/core";
import { zhHans, type MessageKey, type Messages } from "./locales/zh-Hans";
import { zhHant } from "./locales/zh-Hant";
import { en } from "./locales/en";
import {
  LANG_STORAGE_KEY,
  langFromTag,
  navigatorTag,
  readStoredLangPref,
  type Lang,
  type LangPref,
} from "./lang";
import { hantReady, loadHant, translateDynamic } from "./dynamic";

export const MESSAGES: Record<Lang, Messages> = {
  "zh-Hans": zhHans,
  "zh-Hant": zhHant,
  en,
};

let activeLang: Lang = langFromTag(navigatorTag());

/** React 树之外（如 main.tsx 的错误兜底）读取当前语言。 */
export function currentLang(): Lang {
  return activeLang;
}

export function format(
  lang: Lang,
  key: MessageKey,
  vars?: Record<string, string | number>,
): string {
  const s = MESSAGES[lang][key] ?? zhHans[key];
  return vars
    ? s.replace(/\{(\w+)\}/g, (m, k) => (k in vars ? String(vars[k]) : m))
    : s;
}

const isTauri = () =>
  typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

type Ctx = {
  lang: Lang;
  pref: LangPref;
  setPref: (p: LangPref) => void;
  t: (key: MessageKey, vars?: Record<string, string | number>) => string;
  /** 翻译 lib / Rust 产出的运行时中文。 */
  td: (text: string | null | undefined) => string;
};

const I18nCtx = createContext<Ctx | null>(null);

export function I18nProvider({ children }: { children: ReactNode }) {
  const [pref, setPrefState] = useState<LangPref>(readStoredLangPref);
  const [systemTag, setSystemTag] = useState<string>(navigatorTag);
  const [isHantReady, setIsHantReady] = useState<boolean>(hantReady);

  useEffect(() => {
    if (!isTauri()) return;
    invoke<string>("system_locale").then(setSystemTag, () => {});
  }, []);

  const lang: Lang = pref === "system" ? langFromTag(systemTag) : pref;

  useEffect(() => {
    activeLang = lang;
    document.documentElement.lang = lang;
    if (lang === "zh-Hant" && !hantReady()) {
      void loadHant().then(() => setIsHantReady(true));
    }
  }, [lang]);

  useEffect(() => {
    try {
      localStorage.setItem(LANG_STORAGE_KEY, pref);
    } catch {}
    if (isTauri()) {
      void invoke("set_ui_language", { lang: pref }).catch(() => {});
    }
  }, [pref]);

  const setPref = useCallback((p: LangPref) => setPrefState(p), []);
  const value = useMemo<Ctx>(
    () => ({
      lang,
      pref,
      setPref,
      t: (key, vars) => format(lang, key, vars),
      td: (text) => translateDynamic(text, lang),
    }),
    [lang, pref, setPref, isHantReady],
  );
  return <I18nCtx.Provider value={value}>{children}</I18nCtx.Provider>;
}

export function useI18n(): Ctx {
  const c = useContext(I18nCtx);
  if (c) return c;
  return {
    lang: "zh-Hans",
    pref: "system",
    setPref: () => {},
    t: (key, vars) => format("zh-Hans", key, vars),
    td: (text) => text ?? "",
  };
}
