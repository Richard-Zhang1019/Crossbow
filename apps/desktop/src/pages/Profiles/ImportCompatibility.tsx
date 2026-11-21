import {useEffect, useState} from "react";
import {Button} from "@/components/Button";
import {ipc, type ImportInspection} from "@/services/ipc";
import {notify} from "@/notifications/service";
import {subscriptionCleanup} from "@/lib/unlisten";
import {useT} from "@/i18n/i18n";
import "./ImportCompatibility.css";

export default function ImportCompatibility({content,onReady}:{content:string;onReady:(ready:boolean)=>void}) {
  const t=useT();
  const [report,setReport]=useState<ImportInspection|null>(null);
  const [revision,setRevision]=useState(0);
  const [failed,setFailed]=useState(false);
  const [version,setVersion]=useState("—");
  useEffect(()=>subscriptionCleanup(ipc.onEngineChanged(()=>{onReady(false);setRevision(n=>n+1);})),[onReady]);
  useEffect(()=>{
    let current=true;
    onReady(false);setReport(null);setFailed(false);
    // Read-only, local IPC. HTTP URLs are classified without fetching them.
    const timer=setTimeout(()=>void ipc.inspectProfileImport(content).then(value=>{
      if(!current)return;
      setReport(value);onReady(value.status==="compatible"||value.status==="remote");
      void notify.resolve("import-inspection");
    },error=>{if(current){setFailed(true);notify.error(error,"import-inspection");}}),content.trim()?300:0);
    return()=>{current=false;clearTimeout(timer);};
  },[content,revision,onReady]);
  useEffect(()=>{
    if(!report)return;
    let current=true;setVersion("—");
    void ipc.coreBinaryInfoFor(report.engine).then(info=>{if(current)setVersion(info.version||"—");},error=>{if(current)notify.error(error,"import-engine-info");});
    return()=>{current=false;};
  },[report?.engine]);
  return <section className="cb-import-compatibility" aria-label={t("importCaps.title")} aria-busy={!report&&!failed}>
    <div className="cb-import-compatibility-heading"><strong>{t("importCaps.title")}</strong>{report&&<span>{version==="—"?(report.engine==="mihomo"?"mihomo":"sing-box"):version}</span>}</div>
    <p role="status" className="cb-control-note">{t(failed?"importCaps.failed":!report?"importCaps.checking":`importCaps.${report.status}`,{count:report?.node_count??0})}</p>
    {failed&&<Button size="small" onClick={()=>setRevision(n=>n+1)}>{t("importCaps.retry")}</Button>}
    {report&&<>
      <details><summary>{t("importCaps.matrix")}</summary><table><caption>{t("importCaps.baseline",{version:report.tested_version})}</caption><thead><tr><th>{t("importCaps.protocol")}</th><th>mihomo</th><th>sing-box</th></tr></thead>
      <tbody>{report.capabilities.map(row=><tr key={row.protocol}><th scope="row">{row.protocol==="hy2"?"Hy2":row.protocol==="tuic"?"TUIC":"REALITY / vision"}</th><td>{t(`importCaps.${row.mihomo==="unsupported"?"unsupportedTable":row.mihomo}`)}</td><td>{t(`importCaps.${row.singbox==="unsupported"?"unsupportedTable":row.singbox}`)}</td></tr>)}</tbody></table>
      <p className="cb-control-note">{t("importCaps.limits")}</p></details>
    </>}
  </section>;
}
