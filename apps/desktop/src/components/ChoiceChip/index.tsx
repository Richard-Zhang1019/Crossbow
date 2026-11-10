import {ButtonBase} from "@/components/Button";
import {Icon} from "@/components/Icon";
import type {ReactNode,ButtonHTMLAttributes} from 'react';
import './Choices.css';
/** Multi-selection filter. A check and a tinted fill carry selection, not colour alone. */
export function ChoiceChip({selected,children,className='',...props}:ButtonHTMLAttributes<HTMLButtonElement>&{selected:boolean;children:ReactNode}) {
 return <ButtonBase {...props} className={`cb-filter-chip ${className}`} aria-pressed={selected}><span className="cb-filter-check" aria-hidden="true">{selected?<Icon name="check" size={12}/>:null}</span>{children}</ButtonBase>;
}
