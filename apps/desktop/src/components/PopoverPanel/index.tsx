import { useState, type ReactElement, type ReactNode } from "react";
import { Popover } from "@base-ui/react/popover";
import { ButtonBase } from "@/components/Button";
import "./PopoverPanel.css";
export default function PopoverPanel({
  label,
  trigger,
  children,
  onOpen,
  className = "",
}: {
  label: string;
  trigger: ReactElement;
  children: (close: () => void) => ReactNode;
  onOpen?: () => void;
  className?: string;
}) {
  const [open, setOpen] = useState(false),
    [instant, setInstant] = useState(false);
  return (
    <Popover.Root
      open={open}
      onOpenChange={(next) => {
        setOpen(next);
        if (next) onOpen?.();
      }}
      modal={false}
    >
      <Popover.Trigger
        render={trigger}
        aria-label={label}
        onKeyDown={() => setInstant(true)}
        onPointerDown={() => setInstant(false)}
      />
      <Popover.Portal>
        <Popover.Positioner
          side="bottom"
          align="end"
          sideOffset={8}
          collisionPadding={12}
          className="cb-popover-positioner"
        >
          <Popover.Popup
            aria-label={label}
            className={`cb-popover-panel ${className}`}
            data-instant={instant || undefined}
          >
            {children(() => setOpen(false))}
          </Popover.Popup>
        </Popover.Positioner>
      </Popover.Portal>
    </Popover.Root>
  );
}
export function HelpPopover({
  label,
  children,
}: {
  label: string;
  children: ReactNode;
}) {
  return (
    <PopoverPanel
      label={label}
      trigger={<ButtonBase className="cb-help-button">?</ButtonBase>}
    >
      {() => <div className="cb-help-copy">{children}</div>}
    </PopoverPanel>
  );
}
