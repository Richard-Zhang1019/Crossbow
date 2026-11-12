import {
  createContext,
  forwardRef,
  useContext,
  useId,
  useState,
  type InputHTMLAttributes,
  type ReactNode,
  type ButtonHTMLAttributes,
} from "react";
import { ButtonBase } from "@/components/Button";
import { Icon } from "@/components/Icon";
import "./Controls.css";
const ControlLabel = createContext<{ label: string; hint?: string } | null>(
  null,
);
export function ControlRow({
  id,
  label,
  hint,
  children,
}: {
  id?: string;
  label: string;
  hint?: string;
  children?: ReactNode;
}) {
  const labelId = useId(),
    hintId = useId();
  return (
    <ControlLabel.Provider
      value={{ label: labelId, hint: hint ? hintId : undefined }}
    >
      <div
        id={id}
        className="cb-setting-row cb-control-row"
        role="group"
        aria-label={label}
      >
        <div>
          <div id={labelId}>{label}</div>
          {hint && (
            <p id={hintId} title={hint} className="cb-setting-hint">
              {hint}
            </p>
          )}
        </div>
        {children}
      </div>
    </ControlLabel.Provider>
  );
}
export const Input = forwardRef<
  HTMLInputElement,
  InputHTMLAttributes<HTMLInputElement>
>(function Input({ className = "", ...props }, ref) {
  const field = useContext(ControlLabel);
  return (
    <input
      aria-labelledby={props["aria-labelledby"] ?? (props["aria-label"] ? undefined : field?.label)}
      aria-describedby={field?.hint}
      {...props}
      ref={ref}
      className={`cb-input cb-text-input cb-selectable ${className}`}
    />
  );
});
export const SearchField = forwardRef<
  HTMLInputElement,
  InputHTMLAttributes<HTMLInputElement> & { label: string }
>(function SearchField({ label, className = "", ...props }, ref) {
  return (
    <label className={`cb-input-shell ${className}`}>
      <Icon name="search" size={15} />
      <input
        {...props}
        ref={ref}
        aria-label={label}
        className="cb-input-inner cb-selectable"
      />
    </label>
  );
});
export function Switch({
  checked,
  onChange,
  size = "default",
  className = "",
  disabled,
  ...props
}: Omit<ButtonHTMLAttributes<HTMLButtonElement>, "onChange"> & {
  checked: boolean;
  onChange: (value: boolean) => void;
  size?: "default" | "compact";
}) {
  const field = useContext(ControlLabel);
  const [instant, setInstant] = useState(false);
  return (
    <ButtonBase
      {...props}
      role="switch"
      aria-checked={checked}
      aria-labelledby={props["aria-labelledby"] ?? (props["aria-label"] ? undefined : field?.label)}
      aria-describedby={props["aria-describedby"] ?? field?.hint}
      disabled={disabled}
      className={`cb-toggle cb-switch ${checked ? "on" : ""} ${className}`}
      data-size={size}
      data-instant={instant || undefined}
      onKeyDown={(e) => {
        setInstant(true);
        props.onKeyDown?.(e);
      }}
      onPointerDown={(e) => {
        setInstant(false);
        props.onPointerDown?.(e);
      }}
      onClick={event => { props.onClick?.(event); if(!event.defaultPrevented) onChange(!checked); }}
    />
  );
}
