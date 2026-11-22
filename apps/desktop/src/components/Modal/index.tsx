import { createContext, useContext, useId, useLayoutEffect, useRef, useState, useSyncExternalStore, type CSSProperties, type ReactNode } from "react";
import { Dialog } from "@base-ui/react/dialog";
import { Button, type ButtonProps } from "@/components/Button";
import { useT } from "@/i18n/i18n";
import { notify } from "@/notifications/service";
import "./Modal.css";

// Coordinate sibling roots as well as nested dialogs: only the newest open root
// owns modal focus/inert handling. Base UI still implements trapping/restoration.
const ParentModal=createContext<string|null>(null);
let layers: {id:string,parent:string|null}[]=[];
const listeners=new Set<()=>void>();
const subscribe=(listener:()=>void)=>{listeners.add(listener);return()=>{listeners.delete(listener);};};
const snapshot=()=>layers;
function useLayer(id:string,open:boolean){
  const parent=useContext(ParentModal);
  const active=useSyncExternalStore(subscribe,snapshot,snapshot);
  useLayoutEffect(()=>{
    if(!open)return;
    const child=layers.findIndex(layer=>layer.parent===id);
    const next=[...layers];next.splice(child<0?next.length:child,0,{id,parent});
    layers=next;listeners.forEach(fn=>fn());
    return()=>{layers=layers.filter(value=>value.id!==id);listeners.forEach(fn=>fn());};
  },[id,open,parent]);
  return {top:active.at(-1)?.id===id,rank:Math.max(0,active.findIndex(layer=>layer.id===id))};
}

export interface ModalProps {
  open: boolean;
  title: ReactNode;
  children: ReactNode;
  onOk?: () => void | Promise<unknown>;
  onCancel: () => void;
  okText?: ReactNode;
  cancelText?: ReactNode;
  confirmLoading?: boolean;
  okButtonProps?: ButtonProps;
  cancelButtonProps?: ButtonProps;
  footer?: ReactNode;
  width?: number | string;
  centered?: boolean;
  closable?: boolean;
  keyboard?: boolean;
  mask?: { closable?: boolean };
  /** Prevent every user dismissal while business work is committing. */
  closeDisabled?: boolean;
  destroyOnHidden?: boolean;
  afterOpenChange?: (open: boolean) => void;
  afterClose?: () => void;
}

/** Controlled business modal. Successful confirmation never implicitly discards a draft. */
export default function Modal({open,title,children,onOk,onCancel,okText,cancelText,
  confirmLoading=false,okButtonProps,cancelButtonProps,footer,width=560,centered=true,
  closable=true,keyboard=true,mask,closeDisabled=false,destroyOnHidden=true,
  afterOpenChange,afterClose}: ModalProps) {
  const id=useId(),attempt=useRef(0);
  const t=useT(), pending=useRef(false), [confirming,setConfirming]=useState(false);
  const wasOpen=useRef(false);
  useLayoutEffect(()=>{if(open)wasOpen.current=true;},[open]);
  const {top,rank}=useLayer(id,open);
  const popup=useRef<HTMLDivElement>(null);
  useLayoutEffect(()=>{
    // A previously kept-mounted sibling can become top after its initial focus
    // phase. Transfer focus once; ongoing trapping/restoration remains primitive-owned.
    if(!open||!top)return;
    const frame=requestAnimationFrame(()=>{
      if(popup.current&&!popup.current.contains(document.activeElement))popup.current.focus();
    });
    return()=>cancelAnimationFrame(frame);
  },[open,top]);
  const layerStyle={zIndex:`calc(var(--z-modal) + ${rank*2})`};
  const busy=confirmLoading||confirming;
  const locked=busy||closeDisabled;
  const confirm=async()=>{
    if(pending.current||busy||okButtonProps?.disabled||!onOk)return;
    pending.current=true;setConfirming(true);const operation=`modal-confirm-${id}-${++attempt.current}`;
    try {await onOk();} catch(error){notify.error(error,operation);}
    finally {pending.current=false;setConfirming(false);}
  };
  return <ParentModal.Provider value={id}><Dialog.Root open={open} modal={top} disablePointerDismissal={locked||mask?.closable===false}
    onOpenChange={(next,details)=>{
      if(next)return;
      if(!top){details.cancel();return;}
      if(locked||pending.current||(!keyboard&&details.reason==="escape-key")){details.cancel();return;}
      onCancel();
    }} onOpenChangeComplete={next=>{if(next||wasOpen.current)afterOpenChange?.(next);if(!next&&wasOpen.current)afterClose?.();wasOpen.current=next;}}>
    <Dialog.Portal keepMounted={!destroyOnHidden}>
      <Dialog.Backdrop className="cb-modal-mask" style={layerStyle} />
      <Dialog.Viewport className="cb-modal-viewport" style={layerStyle} data-centered={centered}>
        <Dialog.Popup ref={popup} className="cb-modal" onKeyDown={event=>{
          // Sibling Dialog roots have separate dismiss listeners. Handle Escape at
          // the active popup before a lower root's document listener can consume it.
          // Portaled Select popups retain their own Escape handling.
          if(event.key!=="Escape"||event.defaultPrevented||!top||!event.currentTarget.contains(event.target as Node))return;
          event.stopPropagation();event.preventDefault();
          if(keyboard&&!locked&&!pending.current)onCancel();
        }} aria-hidden={!top||undefined} style={{"--cb-modal-width":typeof width==="number"?`${width}px`:width} as CSSProperties} aria-describedby={undefined}>
          <header className="cb-modal-header">
            <Dialog.Title className="cb-modal-title">{title}</Dialog.Title>
            {closable&&<Dialog.Close render={<Button iconOnly size="small" />} disabled={locked} aria-label={t("studio.close")}>×</Dialog.Close>}
          </header>
          <div className="cb-modal-body">{children}</div>
          {footer!==null&&<footer className="cb-modal-footer">{footer===undefined?<>
            <Button {...cancelButtonProps} disabled={locked||cancelButtonProps?.disabled} onClick={onCancel}>{cancelText??t("ov.cancel")}</Button>
            <Button variant="primary" {...okButtonProps} loading={busy} disabled={!onOk||okButtonProps?.disabled} onClick={()=>void confirm()}>{okText??t("pr.save")}</Button>
          </>:footer}</footer>}
        </Dialog.Popup>
      </Dialog.Viewport>
    </Dialog.Portal>
  </Dialog.Root></ParentModal.Provider>;
}
