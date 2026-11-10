import { useState } from "react";
import { ButtonBase } from "@/components/Button";
import { Select as SelectPrimitive } from "@base-ui/react/select";
import { Icon } from "@/components/Icon";
import "./Select.css";

export interface SelectOption {
  value: string | number;
  label: string;
  disabled?: boolean;
}

interface SelectCommonProps {
  options: SelectOption[];
  label: string;
  className?: string;
  size?: "default" | "compact";
  disabled?: boolean;
}
export type SelectProps = SelectCommonProps & (
  | { multiple?: false; value: string | number; onChange: (value: string) => void }
  | { multiple: true; value: readonly (string | number)[]; onChange: (value: string[]) => void; selectionLabel: string }
);

/** One menu system for single and multiple selection, including keyboard and portal behavior. */
export default function Select(props: SelectProps) {
  const { value, options, label, className = "", size = "default", disabled = false } = props;
  const multiple = props.multiple === true;
  const [keyboard, setKeyboard] = useState(false);
  const items = options.map((option) => ({
    ...option,
    value: String(option.value),
  }));
  const current = items.find((option) => option.value === String(value));
  const selectionLabel = props.multiple ? props.selectionLabel : undefined;
  return (
    <div className={`cb-select ${className}`} data-size={size}>
      <SelectPrimitive.Root<string, boolean>
        multiple={multiple}
        items={items}
        value={props.multiple ? props.value.map(String) : String(value)}
        onValueChange={(next) => {
          if (props.multiple) {
            if (Array.isArray(next)) props.onChange(next);
          } else if (typeof next === "string") {
            props.onChange(next);
          }
        }}
        disabled={disabled || items.length === 0}
        modal={false}
      >
        <SelectPrimitive.Trigger
          render={<ButtonBase />}
          className="cb-select-trigger"
          aria-label={label}
          title={selectionLabel ?? current?.label}
          onPointerDown={() => setKeyboard(false)}
          onKeyDown={() => setKeyboard(true)}
          data-instant={keyboard || undefined}
        >
          <SelectPrimitive.Value className="cb-select-value" placeholder="—">
            {selectionLabel}
          </SelectPrimitive.Value>
          <SelectPrimitive.Icon className="cb-select-chevron">
            <Icon name="chevron" size={12} />
          </SelectPrimitive.Icon>
        </SelectPrimitive.Trigger>
        <SelectPrimitive.Portal>
          <SelectPrimitive.Positioner
            className="cb-select-positioner"
            side="bottom"
            align="start"
            sideOffset={6}
            collisionPadding={8}
            alignItemWithTrigger={false}
          >
            <SelectPrimitive.Popup
              className="cb-select-popup"
              data-instant={keyboard || undefined}
              onKeyDown={() => setKeyboard(true)}
            >
              <SelectPrimitive.List className="cb-select-list">
                {items.map((option) => (
                  <SelectPrimitive.Item
                    key={option.value}
                    value={option.value}
                    label={option.label}
                    disabled={option.disabled}
                    className="cb-select-option"
                  >
                    <SelectPrimitive.ItemText className="cb-select-option-text">
                      {option.label}
                    </SelectPrimitive.ItemText>
                    <SelectPrimitive.ItemIndicator className="cb-select-check">
                      <Icon name="check" size={14} strokeWidth={2} />
                    </SelectPrimitive.ItemIndicator>
                  </SelectPrimitive.Item>
                ))}
              </SelectPrimitive.List>
            </SelectPrimitive.Popup>
          </SelectPrimitive.Positioner>
        </SelectPrimitive.Portal>
      </SelectPrimitive.Root>
    </div>
  );
}
