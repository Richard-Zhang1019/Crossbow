import {useCallback,useEffect,useRef,useState} from "react";
import {ipc,type UserRule,type UserRuleStatus} from "@/services/ipc";
import {useT} from "@/i18n/i18n";
import {notify} from "@/notifications/service";
import {Button} from "@/components/Button";
import {Input} from "@/components/Controls";
import Dialog from "@/components/Dialog";
import Select from "@/components/Select";
import "../ProcessRules/ProcessRules.css";
import "./UserRules.css";
// ConnRow.host is an endpoint with a destination port, including bare IPv6.
// Never create suffix rules or broad network ranges implicitly.
function candidate(raw:string):Pick<UserRule,"kind"|"value"> {
  const host=raw.startsWith("[")?raw.slice(1,raw.indexOf("]")):raw.replace(/:\d+$/,"");
  if(host.includes(":")||/^\d+\.\d+\.\d+\.\d+$/.test(host))return {kind:"IP_CIDR",value:host+(host.includes(":")?"/128":"/32")};
  return {kind:"DOMAIN",value:host};
}
export default function UserRules({preset}:{preset?:string}) {
 const t=useT(),alive=useRef(false),revision=useRef(0),pending=useRef(false),draftBase=useRef<UserRule[]|null>(null);
 const [status,setStatus]=useState<UserRuleStatus|null>(null),[failed,setFailed]=useState(false),[busy,setBusy]=useState(false),[draft,setDraft]=useState<UserRule|null>(null);
 // draftBase always mirrors the last loaded server list: the stale-edit guard must
 // compare against the newest state, not the list captured when the page mounted.
 const load=useCallback(async()=>{const seq=++revision.current;try{const next=await ipc.userRules();if(alive.current&&seq===revision.current){draftBase.current=next.rules;setStatus(next);setFailed(false);void notify.resolve("user-rules-read");}}catch(e){if(alive.current&&seq===revision.current){setFailed(true);notify.error(e,"user-rules-read");}}},[]);
 useEffect(()=>{alive.current=true;void load();const off=[ipc.onProfileSwitched(()=>void load()),ipc.onProfileUpdated(()=>void load()),ipc.onEngineChanged(()=>void load())];return()=>{alive.current=false;revision.current++;off.forEach(p=>void p.then(f=>f(),e=>notify.error(e,"user-rules-listener")));};},[load]);
 useEffect(()=>{if(preset)setDraft({id:crypto.randomUUID(),...candidate(preset),target:"DIRECT",enabled:true});},[preset]);
 const openDraft=(rule:UserRule)=>{draftBase.current=status?.rules??null;setDraft(rule);};
 const change=async(rules:UserRule[],expected=status?.rules)=>{
  if(!status||!expected||pending.current||failed)return;pending.current=true;setBusy(true);
  let saved=false;
  try{const result=await notify.run(`user-rule-${crypto.randomUUID()}`,t("ur.title"),()=>ipc.setUserRules(expected,rules));saved=alive.current&&result.ok;if(saved)setDraft(null);}
  finally{pending.current=false;if(alive.current)setBusy(false);}
  // A rejected save is usually a stale list (another writer or a refresh landed
  // mid-edit). Reload so the preserved draft can be retried instead of failing forever.
  if(alive.current)await load();
 };
 const move=(index:number,offset:number)=>{if(!status)return;const rules=[...status.rules];[rules[index],rules[index+offset]]=[rules[index+offset],rules[index]];void change(rules);};
 return <section className="cb-process-rules" aria-label={t("ur.title")}>
  <div className="cb-process-toolbar"><span className="cb-process-count">{t("ur.title")} <span>{status?.rules.length??"—"}</span></span><Button variant="primary" disabled={!status||busy||failed} onClick={()=>openDraft({id:crypto.randomUUID(),kind:"DOMAIN",value:"",target:"DIRECT",enabled:true})}>{t("pr.add")}</Button></div>

  <p className="cb-process-footnote">{t("ur.priority")}</p>
  {failed?<Button onClick={()=>void load()}>{t("home.retryState")}</Button>:!status?<p role="status">{t("workflow.loading")}</p>:status.rules.length===0?<div className="cb-process-empty">{t("ur.empty")}</div>:<div className="cb-card cb-process-table" role="table" aria-label={t("ur.title")}>
   <div className="cb-th cb-table-row cb-user-row" role="row"><span role="columnheader">{t("ur.match")}</span><span role="columnheader">{t("pr.targetLabel")}</span><span role="columnheader">{t("pr.actions")}</span></div>
   {status.rules.map((rule,index)=><div className="cb-table-row cb-process-row cb-user-row" role="row" key={rule.id}>
    <div className="cb-process-identity" role="cell"><small>{index+1} · {rule.kind.replaceAll("_","-")}</small>{rule.value}{status.overlaps?.some(([i])=>i===index)&&<small>{t("ur.overlap")}</small>}</div>
    <div className="cb-process-route" role="cell">{rule.target}{!status.targets.includes(rule.target)&&<small>{t("pr.unavailable")}</small>}</div>
    <div className="cb-process-actions" role="cell"><Button size="small" disabled={busy||index===0} aria-label={t("ur.up")} onClick={()=>move(index,-1)}>↑</Button><Button size="small" disabled={busy||index===status.rules.length-1} aria-label={t("ur.down")} onClick={()=>move(index,1)}>↓</Button><Button size="small" aria-pressed={rule.enabled} disabled={busy} onClick={()=>void change(status.rules.map(r=>r.id===rule.id?{...r,enabled:!r.enabled}:r))}>{t(rule.enabled?"ur.on":"ur.off")}</Button><Button size="small" disabled={busy} onClick={()=>openDraft({...rule})}>{t("pr.edit")}</Button></div>
   </div>)}
  </div>}
  <p className="cb-process-footnote">{t("ur.explain")}</p>
  {draft&&<Dialog title={t("ur.title")} onClose={()=>{if(!pending.current)setDraft(null);}}><form className="cb-process-form" onSubmit={e=>{e.preventDefault();if(status){const expected=draftBase.current??status.rules;const rules=expected.some(r=>r.id===draft.id)?expected.map(r=>r.id===draft.id?draft:r):[...expected,draft];void change(rules,expected);};}}>
   <label>{t("ur.match")}<Select label={t("ur.match")} value={draft.kind} disabled={busy} options={["DOMAIN","DOMAIN_SUFFIX","IP_CIDR"].map(value=>({value,label:value.replaceAll("_","-")}))} onChange={kind=>setDraft({...draft,kind:kind as UserRule["kind"]})}/><Input autoFocus value={draft.value} disabled={busy} maxLength={253} placeholder="example.com / 192.0.2.0/24 / 2001:db8::/64" onChange={e=>setDraft({...draft,value:e.target.value})}/></label>
   <label>{t("pr.targetLabel")}<Select label={t("pr.targetLabel")} value={draft.target} disabled={busy} options={(status?.targets??[]).map(value=>({value,label:value}))} onChange={target=>setDraft({...draft,target})}/></label>
   <p className="cb-control-note">{t("ur.editor")}</p>
   <div className="cb-process-form-actions">{status?.rules.some(r=>r.id===draft.id)&&<Button tone="danger" disabled={busy} onClick={()=>void change((draftBase.current??status.rules).filter(r=>r.id!==draft.id),draftBase.current??status.rules)}>{t("pr.remove")}</Button>}<Button disabled={busy} onClick={()=>setDraft(null)}>{t("pr.cancel")}</Button><Button type="submit" variant="primary" loading={busy} disabled={busy||!status||failed||!draft.value.trim()||!status.targets.includes(draft.target)}>{t("pr.save")}</Button></div>
  </form></Dialog>}
 </section>;
}
