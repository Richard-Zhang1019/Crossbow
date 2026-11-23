import {ButtonBase} from "@/components/Button";
import {Icon} from "@/components/Icon";
import type {ReactNode,ButtonHTMLAttributes} from 'react';
import './Choices.css';
/** A visible checkbox in both states keeps the filter legible without a blank icon slot. */
export function ChoiceChip({selected,children,className='',...props}:ButtonHTMLAttributes<HTMLButtonElement>&{selected:boolean;children:ReactNode}) {
 return <ButtonBase {...props} className={`cb-filter-chip ${className}`} aria-pressed={selected}><span className="cb-filter-check" aria-hidden="true">{selected?<Icon name="check" size={10} strokeWidth={2}/>:null}</span>{children}</ButtonBase>;
}
