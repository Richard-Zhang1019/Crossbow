import { useCallback, useEffect, useRef, useState } from "react";
import { Button } from "@/components/Button";
import { ChoiceChip } from "@/components/ChoiceChip";
import { ProgressRing } from "@/components/ProgressRing";
import { notify } from "@/notifications/service";
import { useT } from "@/i18n/i18n";
import { nodeProbes, type ProbeKind, type ProbeSnapshot, type ProbeReport } from "@/services/nodeProbes";
import type { NodeDelayContext } from "@/services/ipc";
import "./NodeProbes.css";

export default function NodeProbes({ node, context, blocked, historyKey, onBusy }: {
  node: string; context: NodeDelayContext; blocked: boolean; historyKey: string; onBusy: (busy: boolean) => void;
}) {
  const t = useT();
  const [kind, setKind] = useState<ProbeKind>("connectivity");
  const [job, setJob] = useState<ProbeSnapshot | null>(null), [history, setHistory] = useState<ProbeReport[]>([]);
  const [selected, setSelected] = useState<ProbeReport | null>(null), [starting, setStarting] = useState(false), [readError, setReadError] = useState(false), [retry, setRetry] = useState(0), [queuedCancel, setQueuedCancel] = useState(false);
  const cancelBeforeTicket = useRef(false);
  const currentId = useRef<string | null>(null), alive = useRef(false), pending = useRef(false), historyRequest = useRef(0);
  const busy = starting || job?.state === "running" || job?.state === "cancelling";
  const load = useCallback(async () => {
    const request = ++historyRequest.current;
    try { const rows = await nodeProbes.history(node, context); if (alive.current && request === historyRequest.current) setHistory(rows); }
    catch (error) { if (alive.current && request === historyRequest.current) notify.error(error, "node-probe-history"); }
  }, [node, context.revision, context.pid]);
  useEffect(() => {
    alive.current = true; setHistory([]); setSelected(null); void load();
    return () => { alive.current = false; ++historyRequest.current; if (currentId.current) void nodeProbes.cancel(currentId.current).catch(error => notify.error(error, "node-probe-cancel")); };
  }, [load]);
  const previousEpoch = useRef(historyKey);
  useEffect(() => { if (previousEpoch.current !== historyKey) { previousEpoch.current = historyKey; setSelected(null); void load(); } }, [historyKey, load]);
  useEffect(() => { onBusy(busy); return () => onBusy(false); }, [busy, onBusy]);
  useEffect(() => {
    if (!job || !["running", "cancelling"].includes(job.state)) return;
    let stopped = false, timer: ReturnType<typeof setTimeout> | undefined;
    const poll = async () => {
      try {
        const next = await nodeProbes.snapshot(job.id, context);
        if (stopped || !alive.current || currentId.current !== job.id) return;
        if (!next) { currentId.current = null; setJob(null); return; }
        setReadError(false); setJob(next);
        if (!["running", "cancelling"].includes(next.state)) { currentId.current = null; if (next.report) setSelected(next.report); await load(); return; }
        timer = setTimeout(poll, 300);
      } catch (error) {
        if (!stopped && alive.current) { notify.error(error, "node-probe-state"); setReadError(true); }
      }
    };
    void poll();
    return () => { stopped = true; clearTimeout(timer); };
  }, [job?.id, job?.state, context.revision, context.pid, load, retry]);
  const start = async () => {
    if (pending.current || busy || blocked) return;
    pending.current = true; cancelBeforeTicket.current = false; setQueuedCancel(false); setReadError(false); setStarting(true); setSelected(null);
    try {
      const result = await notify.run("node-probe-start", t("probes.title"), () => nodeProbes.start(node, context, kind), undefined, {feedback: "error-only"});
      if (!result.ok) return;
      if (!alive.current) { await notify.run(`node-probe-cancel-${result.value}`, t("probes.cancel"), () => nodeProbes.cancel(result.value), undefined, {feedback:"error-only"}); return; }
      currentId.current = result.value;
      let cancelled = false;
      if (cancelBeforeTicket.current) {
        const stop = await notify.run(`node-probe-cancel-${result.value}`, t("probes.cancel"), () => nodeProbes.cancel(result.value), undefined, {feedback:"error-only"});
        cancelled = stop.ok; if (!stop.ok) setQueuedCancel(false);
      }
      setJob({id:result.value,node,kind,state:cancelled ? "cancelling" : "running",phase:"preparing",report:null,error:null});
    } finally { pending.current = false; if (alive.current) setStarting(false); }
  };
  const cancel = async () => {
    const id = currentId.current; if (!id) { if (pending.current) { cancelBeforeTicket.current = true; setQueuedCancel(true); } return; }
    const result = await notify.run(`node-probe-cancel-${id}`, t("probes.cancel"), () => nodeProbes.cancel(id), undefined, {feedback:"error-only"});
    if (result.ok && alive.current && currentId.current === id) setJob(old => old && ({...old,state:"cancelling"}));
  };
  const ms = (value: number) => `${value} ms`;
  const report = selected?.kind === kind ? selected : history.find(row => row.kind === kind);
  const rows = report?.kind === "connectivity"
    ? [[t("probes.tcp"), report.tcp.class === "ok" ? `${t("probes.ok")} · ${ms(report.tcp.elapsedMs)}` : t(`probes.check.${report.tcp.class}`)],
       [t("probes.https"), report.https.class === "ok" ? `${t("probes.ok")} · ${ms(report.https.elapsedMs)}` : t(`probes.check.${report.https.class}`)],
       [t("probes.udp"), report.udp.class === "ok" ? `${t("probes.ok")} · ${ms(report.udp.elapsedMs)} · ${report.udp.packets} ${t("probes.datagrams")}` : t(`probes.check.${report.udp.class}`)]]
    : report ? [[t("probes.download"), report.download.mbps == null ? t("probes.incomplete") : `${report.download.mbps.toFixed(2)} Mbps · ${(report.download.bytes / 1048576).toFixed(2)} MiB`],
                [t("probes.upload"), report.upload.mbps == null ? t("probes.unconfirmed") : `${report.upload.mbps.toFixed(2)} Mbps · ${(report.upload.bytes / 1048576).toFixed(2)} MiB`]] : [];
  return <section className="cb-node-probes" aria-label={t("probes.title")}>
    <div className="cb-probe-mode" role="group" aria-label={t("probes.mode")}>
      {(["connectivity", "throughput"] as const).map(value => <ChoiceChip key={value} disabled={busy} selected={kind === value} onClick={() => setKind(value)}>{t(`probes.${value}`)}</ChoiceChip>)}<span className="cb-probe-demand">{t("probes.onDemand")}</span>
    </div>
    <p className="cb-probe-budget">{t(kind === "throughput" ? "probes.transferBudget" : "probes.connectBudget")}</p>
    <p className="cb-probe-note">{t("probes.isolation")}</p>
    <div className="cb-probe-action">
      {busy ? <><span role="status"><ProgressRing size={16} value={null} label={t("probes.title")} />{readError ? t("probes.readError") : t(`probes.phase.${job?.state === "cancelling" || queuedCancel ? "cancelling" : job?.phase ?? "preparing"}`)}</span>{readError && <Button size="small" onClick={() => {setReadError(false);setRetry(n => n + 1);}}>{t("probes.retryState")}</Button>}<Button size="small" disabled={job?.state === "cancelling" || queuedCancel} onClick={() => void cancel()}>{t("probes.cancel")}</Button></>
        : <Button size="small" disabled={blocked} onClick={() => void start()}>{t(kind === "connectivity" ? "probes.startConnect" : "probes.startTransfer")}</Button>}
    </div>
    {job && !busy && ["cancelled","expired","failed"].includes(job.state) && <p role="status" className="cb-probe-note">{t(`probes.state.${job.state}`)}</p>}
    {report ? <div className="cb-probe-report"><div className="cb-probe-date">{t(selected?.kind === kind ? "probes.result" : "probes.lastResult")} · {new Date(report.atMs).toLocaleString()} · {(report.elapsedMs / 1000).toFixed(1)} s</div><dl>{rows.map(([label,value]) => <div key={label}><dt>{label}</dt><dd>{value}</dd></div>)}</dl></div> : <p className="cb-probe-note">{t("probes.empty")}</p>}
    <p className="cb-probe-note">{t(kind === "connectivity" ? "probes.connectCaveat" : "probes.transferCaveat")}</p><p className="cb-probe-note">{t("probes.retention")}</p>
    {history.length > 1 && <details className="cb-probe-history"><summary>{t("probes.history")}</summary>{history.map((row,i) => <Button key={`${row.atMs}:${i}`} size="small" disabled={busy} onClick={() => {setSelected(row);setKind(row.kind);}}>{new Date(row.atMs).toLocaleString()} · {t(`probes.${row.kind}`)}</Button>)}</details>}
  </section>;
}
