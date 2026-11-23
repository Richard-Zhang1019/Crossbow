import { forwardRef, useEffect, useId, useState } from "react";
import { Tooltip } from "@base-ui/react/tooltip";
import { Button, type ButtonProps } from "@/components/Button";
import { Icon } from "@/components/Icon";
import "./IconAction.css";

type Props = Omit<ButtonProps,"children"|"iconOnly"|"aria-label"|"title"> & {
  icon: string;
  label: string;
  success?: boolean;
  presentation?: "inline" | "button";
};

/** Quiet auxiliary action: one shared button interaction model, accessible tooltip. */
export const IconAction = forwardRef<HTMLButtonElement,Props>(function IconAction(
  {icon,label,success=false,presentation="inline",size="small",className,disabled,loading,...props},ref,
){
  const id=useId(), [open,setOpen]=useState(false);
  useEffect(()=>{if(disabled||loading)setOpen(false);},[disabled,loading]);
  return <Tooltip.Root open={open&&!disabled&&!loading} onOpenChange={setOpen} disabled={disabled||loading}>
    <Tooltip.Trigger delay={350} aria-describedby={open&&!disabled&&!loading?id:undefined} render={<Button {...props} ref={ref} size={size} iconOnly disabled={disabled} loading={loading}
      aria-label={label} data-presentation={presentation} data-success={success||undefined} className={["cb-icon-action",className].filter(Boolean).join(" ")} />}>
      <Icon name={success?"check":icon} size={16}/>
      <span className="cb-icon-action-status" role="status">{success?label:""}</span>
    </Tooltip.Trigger>
    <Tooltip.Portal><Tooltip.Positioner side="top" sideOffset={6} className="cb-icon-tooltip-positioner">
      <Tooltip.Popup id={id} role="tooltip" className="cb-icon-tooltip">{label}</Tooltip.Popup>
    </Tooltip.Positioner></Tooltip.Portal>
  </Tooltip.Root>;
});
