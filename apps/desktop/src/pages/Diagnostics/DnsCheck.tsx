import { useState, useEffect, useRef, type MutableRefObject } from "react";
import SegmentedControl from "@/components/SegmentedControl";
import { Button, ButtonBase } from "@/components/Button";
import { Input } from "@/components/Controls";
import { Icon } from "@/components/Icon";
import { useClipboardFeedback } from "@/hooks/useClipboardFeedback";
import { notify } from "@/notifications/service";
import { ipc } from "@/services/ipc";
import { currentLang, useT } from "@/i18n/i18n";
import { pathStateId, probeSummary, pushHistory, verdictId, verdictTone, type DnsHistoryEntry } from "./dnsReport";

function probeTime(atMs: number) {
  return new Date(atMs).toLocaleString(currentLang() === "en" ? "en-US" : "zh-CN");
}

/**
 * #603 DNS 解析路径：用户主动触发的一次对比——系统解析器 vs 内核 DNS 接口查询。
 * 只有点击「检查解析」才发起查询；历史仅保留本次会话最近几条，可清除。
 */
export default function DnsCheck({ revision }: { revision: MutableRefObject<number> }) {
  const t = useT();
  const request=useRef(0), pending=useRef(false);
  useEffect(()=>()=>{++request.current;pending.current=false;},[]);
  const cancel=()=>{++request.current;pending.current=false;setRunning(false);};
  const [open, setOpen] = useState(false);
  const [domain, setDomain] = useState("");
  const [recordType, setRecordType] = useState<"A" | "AAAA">("A");
  const [running, setRunning] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [history, setHistory] = useState<DnsHistoryEntry[]>([]);
  const [selected, setSelected] = useState(0);
  const { copied, copy, clear: clearCopy } = useClipboardFeedback<string>("diagnostics-dns-copy");

  const shown = history[selected];
  const stale = !!shown && shown.revision !== revision.current;

  const run = async () => {
    const name = domain.trim();
    if (pending.current || !name) return;
    const id=++request.current;pending.current=true;
    const startedRevision = revision.current;
    setRunning(true);
    setError(null);
    try {
      const probe = await ipc.diagnoseDns(name, recordType);
      if(id!==request.current || revision.current!==startedRevision)return;
      setHistory(old => pushHistory(old, { probe, revision: startedRevision }));
      setSelected(0);
      clearCopy();
      void notify.resolve("diagnostics-dns");
    } catch (failure) {
      if(id!==request.current || revision.current!==startedRevision)return;
      setError(String(failure));
      notify.error(failure, "diagnostics-dns", t("dnsprobe.failed"));
    } finally {
      if(id===request.current){pending.current=false;setRunning(false);}
    }
  };

  // 折叠态摘要只放「查了什么、什么时候」，结论交给右侧状态标，避免同一句显示两遍。
  const summary = shown
    ? `${shown.probe.domain} ${shown.probe.record_type} · ${probeTime(shown.probe.at_ms)}`
    : t("dnsprobe.hint");

  return (
    <section className={`cb-card cb-dnsprobe-check${open ? " open" : ""}`} aria-label={t("dnsprobe.title")}>
      <ButtonBase
        className="cb-dnsprobe-check-toggle"
        aria-expanded={open}
        aria-controls="cb-dnsprobe-check-body"
        onClick={() => setOpen(value => !value)}
      >
        <span className="cb-fault-chev" aria-hidden="true"><Icon name="chevron" size={15} strokeWidth={2} /></span>
        <h2>{t("dnsprobe.title")}</h2>
        <span className="cb-dnsprobe-check-sum">{summary}</span>
        {shown && <span className="cb-dnsprobe-tone" data-state={verdictTone(shown.probe.verdict)}>
          {t(`dnsprobe.verdict.${verdictId(shown.probe.verdict)}`)}
        </span>}
      </ButtonBase>
      <div className="cb-dnsprobe-check-body" id="cb-dnsprobe-check-body" hidden={!open}>
        <form
          className="cb-dnsprobe-form"
          onSubmit={event => { event.preventDefault(); void run(); }}
        >
          <label className="cb-dnsprobe-field">
            <span>{t("dnsprobe.domain")}</span>
            <Input
              value={domain}
              onChange={event => setDomain(event.target.value)}
              placeholder={t("dnsprobe.domainPlaceholder")}
              spellCheck={false}
              autoComplete="off"
              autoCapitalize="off"
              disabled={running}
            />
          </label>
          <div className="cb-dnsprobe-type">
            <span>{t("dnsprobe.type")}</span>
            <SegmentedControl
              label={t("dnsprobe.type")}
              value={recordType}
              options={[{ value: "A", label: "A" }, { value: "AAAA", label: "AAAA" }]}
              onChange={setRecordType}
              disabled={running}
            />
          </div>
          <Button type="submit" variant="primary" loading={running} disabled={!domain.trim()}>
            {running ? t("dnsprobe.running") : t("dnsprobe.run")}
          </Button>
          {running&&<Button onClick={cancel}>{t("pr.cancel")}</Button>}
        </form>
        {error && <p className="cb-dnsprobe-error" role="alert">{error}</p>}
        <p className="cb-dnsprobe-hint">{t("dnsprobe.privacy")}</p>
        {shown && (
          <div className="cb-dnsprobe-result" aria-live="polite">
            <p className="cb-dnsprobe-verdict" data-state={verdictTone(shown.probe.verdict)}>
              {t(`dnsprobe.verdict.${verdictId(shown.probe.verdict)}`)}
            </p>
            <p className="cb-dnsprobe-verdict-hint">
              {t(`dnsprobe.verdictHint.${verdictId(shown.probe.verdict)}`)}
            </p>
            <div className="cb-dnsprobe-paths">
              {([
                ["system", t("dnsprobe.path.system"), shown.probe.system],
                ["kernel", t("dnsprobe.path.kernel"), shown.probe.kernel],
              ] as const).map(([key, label, path]) => {
                const state = pathStateId(path.state);
                return (
                  <div className="cb-dnsprobe-path-row" key={key}>
                    <div className="cb-dnsprobe-path-head">
                      <span className="cb-dnsprobe-path-name">{label}</span>
                      <span className="cb-dnsprobe-pill" data-state={state === "ok" ? "ok" : state === "unavailable" ? "na" : "bad"}>
                        {t(`dnsprobe.state.${state}`)}
                      </span>
                      <span className="cb-dnsprobe-ms">{t("dnsprobe.ms", { n: path.ms })}</span>
                    </div>
                    <div className="cb-dnsprobe-answers">
                      {path.answers.length > 0
                        ? <span className="cb-mono cb-selectable">{path.answers.join(", ")}</span>
                        : <span className="cb-fault-na">—</span>}
                      {path.ttl != null && <span className="cb-dnsprobe-ttl">{t("dnsprobe.ttl", { n: path.ttl })}</span>}
                    </div>
                    {path.note && <p className="cb-dnsprobe-note">{path.note}</p>}
                  </div>
                );
              })}
            </div>
            <p className="cb-dnsprobe-context">
              {t("dnsprobe.context", {
                engine: shown.probe.engine,
                version: shown.probe.core_version ?? t("dnsprobe.versionUnknown"),
                pid: shown.probe.core_pid ?? t("dnsprobe.pidUnknown"),
                mode: t(`dnsprobe.mode.${shown.probe.dns_mode === "custom" ? "custom" : "inherit"}`),
                time: probeTime(shown.probe.at_ms),
              })}
            </p>
            <p className="cb-dnsprobe-context">{t("dnsprobe.profile")}: {shown.probe.profile_id ?? "—"} · {shown.probe.config_revision ?? "—"}</p>
            {shown.probe.hosts_override && <p className="cb-dnsprobe-warn">{t("dnsprobe.hosts")}</p>}
            {stale && <p className="cb-dnsprobe-warn">{t("dnsprobe.stale")}</p>}
            <div className="cb-dnsprobe-unknown">
              <span className="cb-dnsprobe-unknown-label">{t("dnsprobe.unknown")}</span>
              {shown.probe.unknown_fields.map(field => (
                <span className="cb-dnsprobe-unknown-chip" key={field}>{t(`dnsprobe.unknown.${field}`)}</span>
              ))}
              <span className="cb-dnsprobe-unknown-hint">{t("dnsprobe.unknownHint")}</span>
            </div>
            <div className="cb-dnsprobe-actions">
              <Button size="small" onClick={() => void copy(shown.probe.domain + shown.probe.at_ms, () => probeSummary(shown.probe))}>
                <span role="status">{t(copied ? "dnsprobe.copied" : "dnsprobe.copy")}</span>
              </Button>
              {history.length > 0 && (
                <Button size="small" onClick={() => { cancel(); setHistory([]); setSelected(0); clearCopy(); }}>
                  {t("dnsprobe.clear")}
                </Button>
              )}
            </div>
            {history.length > 1 && (
              <ul className="cb-dnsprobe-history" aria-label={t("dnsprobe.history")}>
                {history.map((entry, index) => (
                  <li key={`${entry.probe.at_ms}-${entry.probe.domain}`}>
                    <ButtonBase
                      className={`cb-dnsprobe-history-item${index === selected ? " current" : ""}`}
                      aria-current={index === selected ? "true" : undefined}
                      onClick={() => setSelected(index)}
                    >
                      <span>{probeTime(entry.probe.at_ms)}</span>
                      <span className="cb-mono">{entry.probe.domain}</span>
                      <span className="cb-dnsprobe-history-verdict" data-state={verdictTone(entry.probe.verdict)}>
                        {t(`dnsprobe.verdict.${verdictId(entry.probe.verdict)}`)}
                      </span>
                    </ButtonBase>
                  </li>
                ))}
              </ul>
            )}
          </div>
        )}
      </div>
    </section>
  );
}
