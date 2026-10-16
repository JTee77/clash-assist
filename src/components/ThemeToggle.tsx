import { useI18n } from "../i18n";
import { useTheme, type ThemeMode } from "../hooks/useTheme";
import { PrefDropdown } from "./PrefDropdown";

const RAYS =
  "M8 1v1.6M8 13.4V15M1 8h1.6M13.4 8H15M3.05 3.05l1.13 1.13M11.82 11.82l1.13 1.13M3.05 12.95l1.13-1.13M11.82 4.18l1.13-1.13";

/** 14px 图标，随 currentColor：浅色 = 太阳，深色 = 月亮，跟随系统 = 半日半月。 */
function ThemeIcon({ mode }: { mode: ThemeMode }) {
  return (
    <svg width="14" height="14" viewBox="0 0 16 16" fill="none" aria-hidden="true">
      {mode === "dark" ? (
        <path
          d="M13.2 10.1A5.6 5.6 0 0 1 5.9 2.8a5.6 5.6 0 1 0 7.3 7.3z"
          stroke="currentColor"
          strokeWidth="1.4"
          strokeLinejoin="round"
        />
      ) : (
        <>
          <circle cx="8" cy="8" r="3.6" stroke="currentColor" strokeWidth="1.4" />
          {mode === "system" ? <path d="M8 4.4a3.6 3.6 0 0 1 0 7.2z" fill="currentColor" /> : null}
          <path d={RAYS} stroke="currentColor" strokeWidth="1.3" strokeLinecap="round" />
        </>
      )}
    </svg>
  );
}

/** 主题下拉：收起只显示「日/月 ▾」；菜单里才有 跟随系统 / 浅色 / 深色。 */
export function ThemeToggle() {
  const { theme, setTheme } = useTheme();
  const { t } = useI18n();
  return (
    <PrefDropdown<ThemeMode>
      icon={<ThemeIcon mode={theme} />}
      ariaLabel={t("theme.aria")}
      value={theme}
      onChange={setTheme}
      options={[
        { value: "system", label: t("theme.system") },
        { value: "light", label: t("theme.light") },
        { value: "dark", label: t("theme.dark") },
      ]}
    />
  );
}
