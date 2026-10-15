import { useEffect, useId, useRef, useState, type ReactNode } from "react";

export type PrefOption<V extends string> = { value: V; label: string };

/**
 * 右上角小下拉：收起时永远只显示「图标 ▾」（不显示当前值文字）；
 * 点开菜单才看到完整选项，当前项带 ✓。点外面 / Esc / 选中后收起。
 */
export function PrefDropdown<V extends string>({
  icon,
  ariaLabel,
  value,
  options,
  onChange,
}: {
  icon: ReactNode;
  ariaLabel: string;
  value: V;
  options: PrefOption<V>[];
  onChange: (v: V) => void;
}) {
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);
  const menuId = useId();
  const current = options.find((o) => o.value === value) ?? options[0]!;

  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (!rootRef.current?.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    document.addEventListener("mousedown", onDown);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDown);
      document.removeEventListener("keydown", onKey);
    };
  }, [open]);

  return (
    <div className="pref-dd" ref={rootRef}>
      <button
        type="button"
        className={`pref-dd-btn${open ? " open" : ""}`}
        aria-label={`${ariaLabel}: ${current.label}`}
        title={`${ariaLabel}: ${current.label}`}
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-controls={menuId}
        onClick={() => setOpen((v) => !v)}
      >
        <span className="pref-dd-icon" aria-hidden="true">
          {icon}
        </span>
        <span className="pref-dd-caret" aria-hidden="true">
          ▾
        </span>
      </button>
      {open ? (
        <ul className="pref-dd-menu" id={menuId} role="listbox" aria-label={ariaLabel}>
          {options.map((o) => (
            <li key={o.value} role="presentation">
              <button
                type="button"
                role="option"
                aria-selected={o.value === value}
                className={`pref-dd-item${o.value === value ? " active" : ""}`}
                onClick={() => {
                  onChange(o.value);
                  setOpen(false);
                }}
              >
                <span className="pref-dd-check" aria-hidden="true">
                  {o.value === value ? "✓" : ""}
                </span>
                {o.label}
              </button>
            </li>
          ))}
        </ul>
      ) : null}
    </div>
  );
}
