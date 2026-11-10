import {useState,type CSSProperties} from 'react';
import {ButtonBase} from "@/components/Button";
import "@/components/ChoiceChip/Choices.css";
export default function SegmentedControl<T extends string>({label,value,options,onChange,disabled=false,className=''}:{label:string;value:T;options:readonly {value:T;label:string;disabled?:boolean}[];onChange:(value:T)=>void;disabled?:boolean;className?:string}) {
 const [instant,setInstant]=useState(false);
 const index=options.findIndex(o=>o.value===value);
 return <div className={`cb-segments ${className}`} role="radiogroup" aria-label={label} data-instant={instant||undefined} data-unknown={index<0||undefined} style={{'--segment-count':options.length,'--segment-index':Math.max(0,index)} as CSSProperties} onPointerDown={()=>setInstant(false)} onKeyDown={e=>{
  setInstant(true);
  if(!['ArrowLeft','ArrowRight','ArrowUp','ArrowDown','Home','End'].includes(e.key))return;
  e.preventDefault();if(disabled)return;
  const enabled=options.filter(o=>!o.disabled);if(!enabled.length)return;
  const current=enabled.findIndex(o=>o.value===value),step=['ArrowLeft','ArrowUp'].includes(e.key)?-1:1;
  const next=e.key==='Home'?enabled[0]:e.key==='End'?enabled[enabled.length-1]:enabled[(current+step+enabled.length)%enabled.length];
  const button=[...e.currentTarget.querySelectorAll<HTMLButtonElement>('button')][options.indexOf(next)];button?.focus();onChange(next.value);
 }}>
 {options.map((o,i)=><ButtonBase key={o.value} role="radio" aria-checked={o.value===value} tabIndex={o.value===value||(index<0&&i===0)?0:-1} disabled={disabled||o.disabled} onClick={()=>onChange(o.value)}>{o.label}</ButtonBase>)}
 </div>
}
