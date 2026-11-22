import { useCallback, useEffect, useRef, useState } from "react";
import { Button } from "@/components/Button";
import { Switch } from "@/components/Controls";
import Dialog from "@/components/Dialog";
import Select from "@/components/Select";
import { ChoiceChip } from "@/components/ChoiceChip";
import { Icon } from "@/components/Icon";
import { nodeNameText } from "@/components/NodeName/nodeFlags";
import { notify } from "@/notifications/service";
import { useT } from "@/i18n/i18n";
import { ipc, type NodeDelayContext, type NodeQualityReport, type NodeQualityTarget, type NodeView } from "@/services/ipc";
import "./NodeQuality.css";

const targets: [NodeQualityTarget, string][] = [["google-http", "Google · HTTP"], ["google-https", "Google · HTTPS"], ["cloudflare-https", "Cloudflare · HTTPS"]];
export default function NodeQuality({nodes, initialNode, context, testing, onTest, onCancel, onClose}: {
  nodes: NodeView[]; initialNode: string | null; context: NodeDelayContext; testing: boolean;
  onTest: (node: string, target: NodeQualityTarget) => void; onCancel: () => void; onClose: () => void;
}) {
  const t = useT();
  const [node, setNode] = useState(() => nodes.some(n => n.name === initialNode) ? initialNode! : nodes[0].name);
  const [target, setTarget] = useState<NodeQualityTarget>("google-http");
  const [report, setReport] = useState<NodeQualityReport | null>(null);
  const [loading, setLoading] = useState(true), [failed, setFailed] = useState(false);
  const [saving, setSaving] = useState(false), [confirm, setConfirm] = useState(false);
  const alive = useRef(false), revision = useRef(0), pending = useRef(false), initialized = useRef(false);
  const load = useCallback(async (reportStorageError = true) => {
    const request = ++revision.current;
    setLoading(true);
    try {
      const next = await ipc.nodeQualityReport(node, context);
      if (!alive.current || request !== revision.current) return;
      setReport(next); setFailed(false);
      if (!initialized.current) {
        setTarget(next.targets.find(row => row.total > 0)?.target ?? next.testTargets[0]);
        initialized.current = true;
      }
      if (next.storageError && reportStorageError) notify.error(next.storageError, "node-quality-storage", t("proxies.quality.saveFailed"));
    } catch (error) {
      if (alive.current && request === revision.current) { setFailed(true); notify.error(error, "node-quality-read"); }
    } finally { if (alive.current && request === revision.current) setLoading(false); }
  }, [node, context.revision, context.pid]);
  useEffect(() => {
    alive.current = true;
    setReport(null); initialized.current = false;
    void load();
    return () => { alive.current = false; ++revision.current; };
  }, [load]);
  const wasTesting = useRef(testing);
  useEffect(() => {
    if (wasTesting.current && !testing) void load();
    wasTesting.current = testing;
  }, [testing, load]);
  const change = async (action: () => Promise<void>) => {
    if (pending.current) return;
    pending.current = true; setSaving(true);
    try {
      const result = await notify.run("node-quality-control", t("proxies.quality.title"), action, undefined, {feedback: "error-only"});
      if (alive.current) { if (result.ok) setConfirm(false); await load(result.ok); }
    } finally { pending.current = false; if (alive.current) setSaving(false); }
  };
  const summary = report?.targets.find(row => row.target === target);
  const ms = (value: number | null | undefined) => value == null ? "—" : `${value} ms`;
  return <Dialog title={t("proxies.quality.title")} onClose={onClose}>
    <div className="cb-node-quality">
      <div className="cb-quality-toolbar">
        <Select label={t("proxies.quality.node")} value={node} disabled={saving || testing} onChange={value => {setNode(value); setReport(null); setConfirm(false);}}
          options={nodes.map(n => ({value: n.name, label: nodeNameText(n.name)}))} />
        <Button iconOnly aria-label={t("proxies.quality.refresh")} title={t("proxies.quality.refresh")} onClick={() => void load()} disabled={loading || saving}><Icon name="refresh" /></Button>
        {testing ? <Button onClick={onCancel}>{t("proxies.cancelTest")}</Button> : <Button variant="primary" disabled={saving || !report?.testTargets.includes(target)} onClick={() => onTest(node, target)}>{t("proxies.quality.test")}</Button>}
      </div>
      <p className="cb-quality-hint">{t("proxies.quality.hint")}</p>
      <div className="cb-quality-targets" role="group" aria-label={t("proxies.quality.title")}>
        {targets.map(([value, label]) => <ChoiceChip key={value} disabled={testing} selected={target === value} onClick={() => setTarget(value)}>{label}</ChoiceChip>)}
      </div>
      {report && !report.testTargets.includes(target) && <p className="cb-quality-hint">{t("proxies.quality.targetUnsupported")}</p>}
      <div aria-busy={loading} className="cb-quality-content">
        {failed ? <p role="status">{t("proxies.quality.loadFailed")}</p> : !report || !summary ? <p role="status">{t("proxies.quality.loading")}</p> : <>
          <dl className="cb-quality-metrics">
            <div><dt>{t("proxies.quality.success")}</dt><dd>{summary.successRate == null ? "—" : `${Math.round(summary.successRate * 100)}%`}</dd></div>
            <div><dt>{t("proxies.quality.p50")}</dt><dd>{ms(summary.p50Ms)}</dd></div>
            <div><dt>{t("proxies.quality.p95")}</dt><dd>{ms(summary.p95Ms)}</dd></div>
            <div><dt>{t("proxies.quality.jitter")}</dt><dd>{ms(summary.jitterMs)}</dd></div>
          </dl>
          <div className="cb-quality-sample-note" role="status">
            <strong>{t("proxies.quality.samples", {n:summary.eligible, ok:summary.successes})}</strong>
            <span>{summary.total === 0 ? t("proxies.quality.empty") : summary.eligible < 5 ? t("proxies.quality.small") : t("proxies.quality.errors", {timeout:summary.timeouts, failed:summary.failures, controller:summary.controllerErrors})}</span>
            {summary.total > 0 && summary.eligible < 5 && <span>{t("proxies.quality.errors", {timeout:summary.timeouts, failed:summary.failures, controller:summary.controllerErrors})}</span>}
          </div>
          {summary.samples.length > 0 && <section className="cb-quality-recent" aria-label={t("proxies.quality.recent")}>
            <h3>{t("proxies.quality.recent")}</h3>
            <table><thead><tr><th>{t("proxies.quality.time")}</th><th>{t("proxies.quality.result")}</th><th>{t("proxies.quality.delay")}</th></tr></thead>
              <tbody>{summary.samples.map((sample,i) => <tr key={`${sample.atMs}:${i}`}><td>{new Date(sample.atMs).toLocaleString()}</td><td>{t(`proxies.quality.${sample.class}`)}</td><td>{ms(sample.delayMs)}</td></tr>)}</tbody>
            </table>
          </section>}
          <p className="cb-quality-hint">{t("proxies.quality.policy")}</p>
          <p className="cb-quality-hint">{t("proxies.quality.method")}</p>
          <p className="cb-quality-untested">{t("proxies.quality.untested")}</p>
          {report.storageError && <p role="status" className="cb-quality-storage">{t("proxies.quality.storageError")}</p>}
        </>}
      </div>
      <footer className="cb-quality-controls">
        <div className="cb-quality-record"><span>{t("proxies.quality.record")}</span><Switch checked={!!report && !report.paused} disabled={!report || saving} aria-label={t("proxies.quality.record")} onChange={enabled => void change(() => ipc.nodeQualitySetPaused(!enabled))} /></div>
        <div className="cb-quality-retention"><span>{report && t("proxies.quality.retention", {days:report.retentionDays, count:report.storedSamples, max:report.capacity})}</span>
          <Button size="small" disabled={!report || saving} onClick={() => setConfirm(value => !value)}>{t("proxies.quality.clear")}</Button></div>
        {confirm && <div className="cb-quality-confirm"><p>{t("proxies.quality.confirm")}</p><Button size="small" disabled={saving} onClick={() => setConfirm(false)}>{t("proxies.quality.cancel")}</Button><Button size="small" tone="danger" loading={saving} onClick={() => void change(ipc.nodeQualityClear)}>{t("proxies.quality.confirmClear")}</Button></div>}
      </footer>
    </div>
  </Dialog>;
}
