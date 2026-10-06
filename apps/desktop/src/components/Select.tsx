import { useEffect, useRef, useState } from "react";

export interface SelectOption {
  value: string | number;
  label: string;
}

/** 设计系统风格的迷你下拉（替代原生 select，弹层用 cb tokens）。 */
export default function Select({
  value,
  options,
  onChange,
  className = "",
}: {
  value: string | number;
  options: SelectOption[];
  onChange: (v: string) => void;
  className?: string;
}) {
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);
  const current = options.find((o) => o.value === value);

  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (!rootRef.current?.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener("mousedown", onDown);
    return () => document.removeEventListener("mousedown", onDown);
  }, [open]);

  return (
    <div ref={rootRef} className={`relative ${className}`}>
      <button
        type="button"
        onClick={() => setOpen((o) => !o)}
        className="cb-input inline-flex items-center gap-1.5"
        style={{ width: "auto" }}
      >
        {current?.label ?? "—"}
        <svg
          width="9"
          height="9"
          viewBox="0 0 10 10"
          style={{
            transform: open ? "rotate(180deg)" : "none",
            transition: "transform .12s",
          }}
        >
          <path
            d="M2 3.5 L5 6.5 L8 3.5"
            fill="none"
            stroke="var(--cb-text-dim)"
            strokeWidth="1.4"
            strokeLinecap="round"
          />
        </svg>
      </button>
      {open && (
        <div
          className="cb-card absolute right-0 z-20 mt-1 w-max max-w-[300px] min-w-[110px] overflow-hidden p-1"
          style={{ boxShadow: "0 8px 24px rgba(0,0,0,.4)" }}
        >
          {options.map((o) => (
            <button
              key={o.value}
              onClick={() => {
                onChange(String(o.value));
                setOpen(false);
              }}
              className="block w-full rounded-md px-2.5 py-1.5 text-left text-xs"
              style={{
                color:
                  o.value === value ? "var(--cb-accent)" : "var(--cb-text-dim)",
                background:
                  o.value === value ? "var(--cb-accent-soft)" : "transparent",
              }}
            >
              {o.label}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
