import { useI18n, type LangPref } from "../i18n";
import { PrefDropdown } from "./PrefDropdown";

const GLYPH_BY_PREF: Record<Exclude<LangPref, "system">, string> = {
  "zh-Hans": "简",
  "zh-Hant": "繁",
  en: "EN",
};

/** 语言下拉：跟随系统显示「地球 ▾」，手动选定后切换为等尺寸「简 / 繁 / EN ▾」。 */
export function LanguageToggle() {
  const { pref, setPref, t } = useI18n();
  return (
    <PrefDropdown<LangPref>
      icon={<LangIcon pref={pref} />}
      ariaLabel={t("lang.aria")}
      value={pref}
      onChange={setPref}
      // 语言名永远用各自文字书写（不随界面语言翻译/转换）
      options={[
        { value: "system", label: t("lang.system") },
        { value: "zh-Hans", label: "简体" },
        { value: "zh-Hant", label: "繁體" },
        { value: "en", label: "English" },
      ]}
    />
  );
}

function LangIcon({ pref }: { pref: LangPref }) {
  if (pref !== "system") {
    return (
      <span
        className={`pref-dd-glyph${pref === "en" ? " is-latin" : ""}`}
        aria-hidden="true"
      >
        {GLYPH_BY_PREF[pref]}
      </span>
    );
  }
  return (
    <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.3" strokeLinecap="round" aria-hidden="true">
      <circle cx="8" cy="8" r="6.2" />
      <ellipse cx="8" cy="8" rx="2.6" ry="6.2" />
      <path d="M1.8 8h12.4M2.8 4.8h10.4M2.8 11.2h10.4" />
    </svg>
  );
}
