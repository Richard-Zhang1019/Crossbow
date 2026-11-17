import Dialog from "@/components/Dialog";
import { useClipboardFeedback } from "@/hooks/useClipboardFeedback";
import { Icon } from "@/components/Icon";
import { Button } from "@/components/Button";
import { notify, notificationText } from "@/notifications/service";
import { useOperationNotice } from "@/notifications/NotificationProvider";
import { Fragment, useEffect, useRef, useState, type MutableRefObject, type ReactNode } from "react";
import { ipc, type CheckResult, type FaultEvidence, type FaultEvidenceLayer, type FaultEvidenceProbe, type FaultEvidenceHistory } from "@/services/ipc";
import { currentLang, useT } from "@/i18n/i18n";
import { PageHead } from "@/pages/Home";
import { ProgressRing } from "@/components/ProgressRing";
import { DIAG_STEPS, navigateTo } from "@/lib/workflow";
import { dedupeObservedTargets, failedChecks, faultEvidenceState, faultEvidenceText, faultLayerStats, faultProbeText, guideUrl, isStale, probeOk, remainingAge, shareText, type DiagnosisSnapshot } from "./report";

let previousReport: DiagnosisSnapshot | null = null;

function faultTime(atMs: number) {
  return new Date(atMs).toLocaleString(currentLang() === "en" ? "en-US" : "zh-CN");
}

function FaultState({ fault }: { fault: FaultEvidence }) {
  const t = useT();
  const state = faultEvidenceState(fault);
  return <span className={state === "ongoing" ? "cb-fault-state ongoing" : "cb-fault-state"} role="status">
    {t(`fault.${state}`)}
  </span>;
}

function ProbePill({ probe, fallback, text }: { probe: FaultEvidenceProbe | null; fallback?: string; text: (key: string) => string }) {
  if (!probe) return <span className="cb-fault-na">{fallback ?? "—"}</span>;
  const ok = probeOk(probe);
  return <span className={`cb-fault-pill ${ok ? "ok" : "bad"}`}>{faultProbeText(probe, text)}</span>;
}

function FaultSamples({ fault }: { fault: FaultEvidence }) {
  const t = useT();
  const layerText = (layer: FaultEvidenceLayer) =>
    `${t("fault.primary")} ${faultProbeText(layer.primary, t)}` +
    (layer.review ? ` · ${t("fault.review")} ${faultProbeText(layer.review, t)}` : "");
  const layers: { key: string; ok: boolean; primary: ReactNode; review: ReactNode }[] = [
    {
      key: "mixedPort",
      ok: fault.mixedPortOk,
      primary: <span className={`cb-fault-pill ${fault.mixedPortOk ? "ok" : "bad"}`}>{fault.mixedPortOk ? t("fault.class.ok") : t("fault.class.connect")}</span>,
      review: <span className="cb-fault-na">—</span>,
    },
    {
      key: "controller",
      ok: probeOk(fault.controller),
      primary: <ProbePill probe={fault.controller} text={t} />,
      review: <span className="cb-fault-na">—</span>,
    },
    ...([["directCn", fault.directCn], ["directGlobal", fault.directGlobal], ["proxyHttps", fault.proxyHttps]] as const).map(([key, layer]) => ({
      key,
      ok: probeOk(layer.primary) && (!layer.review || probeOk(layer.review)),
      primary: <ProbePill probe={layer.primary} text={t} />,
      review: <ProbePill probe={layer.review ?? null} text={t} />,
    })),
  ];
  return <>
    <dl className="cb-fault-probe-grid">
      <div className="cb-fault-grid-h">{t("fault.layerHeader")}</div>
      <div className="cb-fault-grid-h">{t("fault.primary")}</div>
      <div className="cb-fault-grid-h">{t("fault.review")}</div>
      {layers.map(layer => (
        <Fragment key={layer.key}>
          <div className="cb-fault-grid-c cb-fault-layer-name">{t(`fault.${layer.key}`)}</div>
          <div className="cb-fault-grid-c">{layer.primary}</div>
          <div className="cb-fault-grid-c">{layer.review}</div>
        </Fragment>
      ))}
    </dl>
    {fault.recovery && <p className="cb-fault-time">
      {t("fault.recoveryProbe")}：{layerText(fault.recovery.proxyHttps)} · {faultTime(fault.recovery.atMs)}
    </p>}
    {fault.legacy && <p className="cb-fault-time">
      {fault.resolvedAtMs != null && !fault.recovery && <>{t("fault.resolvedAt")}：{faultTime(fault.resolvedAtMs)} · </>}
      {t("fault.legacyNote")}
    </p>}
    <p className="cb-fault-time">{t("fault.pathUnverified")}</p>
    <ObservedList observed={fault.sysproxyObserved} />
  </>;
}

/** 观测接口按端点去重；多目标或解析失败时回退原始列表。 */
function ObservedList({ observed }: { observed: string[] }) {
  const t = useT();
  if (!observed.length) return null;
  const groups = dedupeObservedTargets(observed);
  if (!groups) {
    return <p className="cb-fault-observed cb-selectable">
      {t("fault.observed")}：{observed.join("；")}
    </p>;
  }
  return <div className="cb-fault-observed">
    <p className="cb-fault-time">{t("fault.observed")}：{t("fault.observedAll", { n: observed.length })}</p>
    <div className="cb-fault-ifchips">
      {groups.map(group => (
        <span className="cb-fault-ifchip" key={group.iface}>
          <b>{group.count > 1 ? `${group.iface} ×${group.count}` : group.iface}</b>
          <span className="cb-fault-arrow" aria-hidden="true">→</span>
          <code>{group.target}</code>
        </span>
      ))}
    </div>
  </div>;
}

export default function DiagnosticsPage({
  revision, listeners, appVersion,
}: {
  revision: MutableRefObject<number>;
  listeners: Promise<void> | null;
  appVersion: string;
}) {
  const t = useT();
  const [report, setReport] = useState(previousReport);
  const [, refreshAge] = useState(0);
  const [progress, setProgress] = useState<CheckResult[]>([]);
  const [running, setRunning] = useState(false);
  const [runFailed, setRunFailed] = useState(false);
  const [checking, setChecking] = useState<string | null>(null);
  const [fixing, setFixing] = useState<CheckResult | null>(null);
  const [summaryOpen, setSummaryOpen] = useState(false);
  // 故障取证默认折叠：它是背景资料，不应把诊断结果顶出视口（#诊断页改版）
  const [faultOpen, setFaultOpen] = useState(false);
  const [faultHistory, setFaultHistory] = useState<FaultEvidenceHistory | null>(null);
  const { copied, copy, clear } = useClipboardFeedback<string>("diagnostics-summary-copy");
  const restartRunning = useOperationNotice("core-restart")?.kind === "progress";
  const repairRunning = !!fixing || restartRunning;
  const repairLabel = restartRunning || fixing?.fix === "restart_core"
    ? notificationText("core.restarting")
    : fixing?.fix === "start_core" ? t("diag.startingCore") : t("diag.switchingProxy");
  const fixingRef = useRef(false);
  const active = useRef(true);
  const runId = useRef<string | null>(null);

  useEffect(() => {
    active.current = true;
    return () => {
      active.current = false;
      runId.current = null;
    };
  }, []);

  // 更新事件使打开的诊断页同步恢复/清理结果；序号阻止迟到读取覆盖新状态。
  useEffect(() => {
    let cancelled = false;
    let request = 0;
    let offs: (() => void)[] = [];
    const load = () => {
      const id = ++request;
      ipc.sysproxyFaultEvidence()
        .then(evidence => {
          if (!cancelled && id === request) {
            setFaultHistory(evidence);
            void notify.resolve("diagnostics-fault");
          }
        })
        .catch(error => {
          if (!cancelled && id === request) {
            setFaultHistory(null);
            notify.error(error, "diagnostics-fault");
          }
        });
    };
    for (const register of [ipc.onFaultEvidenceUpdated, ipc.onSysproxyStatus, ipc.onCoreStatus]) {
      register(load).then(off => { if (cancelled) off(); else offs.push(off); })
        .catch(error => { if (!cancelled) notify.error(error, "diagnostics-fault"); });
    }
    load();
    window.addEventListener("focus", load);
    return () => {
      cancelled = true;
      ++request;
      offs.forEach(off => off());
      window.removeEventListener("focus", load);
    };
  }, []);

  const stale = !!report && isStale(report, revision.current);
  useEffect(() => {
    if (!report || stale) return;
    const refresh = () => refreshAge(age => age + 1);
    const timer = window.setTimeout(refresh, remainingAge(report));
    window.addEventListener("focus", refresh);
    return () => {
      window.clearTimeout(timer);
      window.removeEventListener("focus", refresh);
    };
  }, [report, stale]);

  const run = async () => {
    if (!active.current || runId.current || !listeners) return;
    const id = crypto.randomUUID();
    runId.current = id;
    setRunning(true);
    setRunFailed(false);
    setProgress([]);
    const runRevision = revision.current;
    const isActiveRun = () => active.current && runId.current === id;
    let off: (() => void) | undefined;
    try {
      await listeners;
      if (!isActiveRun()) return;
      off = await ipc.onDiagnosisProgress(p => {
        if (!isActiveRun() || p.run_id !== id || revision.current !== runRevision) return;
        if (p.result) {
          const row = p.result;
          setProgress(old => [...old.filter(r => r.id !== p.id), row]);
        } else {
          setChecking(p.id);
        }
      });
      if (!isActiveRun()) return;
      if (revision.current !== runRevision) {
        setRunFailed(true);
        return;
      }
      const result = await ipc.runDiagnosis(id);
      if (!isActiveRun()) return;
      if (revision.current !== runRevision) {
        setRunFailed(true);
        return;
      }
      const snapshot = { ...result, completedAt: Date.now(), revision: runRevision };
      previousReport = snapshot;
      setReport(snapshot);
      void notify.resolve("diagnostics-operation");
    } catch (error) {
      if (isActiveRun()) {
        setRunFailed(true);
        notify.error(error, "diagnostics-operation");
      }
    } finally {
      off?.();
      if (isActiveRun()) {
        runId.current = null;
        setChecking(null);
        setRunning(false);
      }
    }
  };

  const fix = async (row: CheckResult) => {
    if (!row.fix || !report || isStale(report, revision.current)
      || fixingRef.current || runId.current || restartRunning) return;
    fixingRef.current = true;
    setFixing(row);
    try {
      if (row.fix === "start_core") {
        const [profile, engine] = await Promise.all([ipc.activeProfileId(), ipc.getEngineConfig()]);
        if (!active.current || isStale(report, revision.current)) return;
        if (!profile || !engine.core_installed) {
          navigateTo({ page: "home", control: "cb-first-connection" });
          return;
        }
        await ipc.coreStart();
      }
      if (row.fix === "restart_core") await ipc.coreRestart();
      if (row.fix === "enable_sysproxy") await ipc.diagnosisEnableSysproxy();
      void notify.resolve("diagnostics-fix");
      await run();
    } catch (error) {
      if (row.fix === "start_core") notify.error(error, "diagnostics-fix");
    } finally {
      fixingRef.current = false;
      if (active.current) setFixing(null);
    }
  };

  const results = running ? progress : report?.rows;
  const failed = failedChecks(results ?? []);
  const first = failed[0];
  const applicable = results?.some(row => !row.skipped);
  const canRepair = !running && !runFailed && !stale;
  const fault = faultHistory?.latest;
  const previousFault = faultHistory?.previous;
  const faultStats = fault ? faultLayerStats(fault) : null;
  const summaryText = summaryOpen ? [
    report ? shareText(report, appVersion, stale || runFailed) : null,
    fault ? faultEvidenceText(fault) : null,
    previousFault ? `Previous fault record\n${faultEvidenceText(previousFault)}` : null,
  ].filter(Boolean).join("\n\n") : null;
  const resultTitle = running
    ? t("diag.progress", { done: progress.length, total: DIAG_STEPS.length })
    : runFailed ? t("diag.incomplete")
    : stale ? t("diag.stale")
    : first ? t("diag.resultIssues", { n: failed.length })
    : applicable ? t("diag.applicableOk") : t("diag.title");
  const goConfig = () => window.dispatchEvent(new CustomEvent("cb:goto", { detail: "profiles" }));
  const faultCard = fault && (
    <section className={`cb-card cb-fault-evidence${faultOpen ? " open" : ""}`} aria-label={t("fault.title")}>
      <button
        type="button"
        className="cb-fault-evidence-toggle"
        aria-expanded={faultOpen}
        aria-controls="cb-fault-evidence-detail"
        onClick={() => setFaultOpen(open => !open)}
      >
        <span className="cb-fault-chev" aria-hidden="true"><Icon name="chevron" size={15} strokeWidth={2} /></span>
        <h2>{t("fault.title")}</h2>
        <span className="cb-fault-evidence-sum">
          {t("fault.startedAt")} {faultTime(fault.startedAtMs)}
          {faultStats && <> · {t("fault.probeLayers", { n: faultStats.total })} · {t("fault.probeFailed", { n: faultStats.failed })}</>}
          {previousFault && <> · {t("fault.previous")} {faultTime(previousFault.startedAtMs)}</>}
        </span>
        <FaultState fault={fault} />
      </button>
      <div className="cb-fault-detail" id="cb-fault-evidence-detail" hidden={!faultOpen}>
        <FaultSamples fault={fault} />
        {previousFault && (
          <details className="cb-fault-prev">
            <summary>{t("fault.previous")} · {faultTime(previousFault.startedAtMs)}</summary>
            <FaultState fault={previousFault} />
            <FaultSamples fault={previousFault} />
          </details>
        )}
      </div>
    </section>
  );
  const statusBand = (running || report) && (
    <div className="cb-diag-summary" aria-live="polite">
      {running ? (
        <ProgressRing value={progress.length / DIAG_STEPS.length * 100} label={resultTitle} size={28} />
      ) : (
        <span className="cb-diagnostic-result" data-result={canRepair && applicable && !first ? "passed" : "attention"} aria-hidden="true">
          <Icon name={canRepair && applicable && !first ? "check" : "activity"} size={20} />
        </span>
      )}
      <div className="cb-diag-band-text">
        <h2>
          {resultTitle}
          {fault && faultEvidenceState(fault) === "ongoing" && (
            <span className="cb-diag-band-chip">{t("diag.ongoingFaultChip")}</span>
          )}
        </h2>
        {!running && report && (
          <p>
            {t("diag.checkedAt", { time: new Date(report.completedAt).toLocaleString() })}
            {canRepair && first && <> · {t("diag.firstBlockerMeta", { check: t(`check.${first.id}`) })}</>}
          </p>
        )}
      </div>
      <div className="cb-diag-band-actions">
        {(report || fault) && !running && (
          <Button onClick={() => setSummaryOpen(true)}>{t("diag.summaryPreview")}</Button>
        )}
        <Button variant="primary" disabled={!listeners} loading={running || repairRunning} onClick={() => void run()}>
          <span role="status">{running ? t("diag.running") : repairRunning ? repairLabel : t(stale || runFailed ? "diag.rerun" : "diag.run")}</span>
        </Button>
      </div>
    </div>
  );
  return (
    <div className="cb-diagnostics-page">
      <PageHead title={t("diag.title")} meta={t("diag.hint")} />
      {!results && !runFailed ? (
        <>
          {faultCard}
          <div className="cb-empty">
            <span className="cb-empty-sig" aria-hidden="true"><Icon name="pulse" size={20} /></span>
            <h2>{t("diag.emptyTitle")}</h2>
            <p>{repairRunning ? repairLabel : t("diag.emptyHint")}</p>
            <Button variant="primary" disabled={!listeners} loading={running} onClick={() => void run()}>
              <span role="status">{repairRunning ? repairLabel : t("diag.run")}</span>
            </Button>
          </div>
        </>
      ) : (
        <>
          {statusBand}
          {faultCard}
          <div className="cb-diag-results">
            {DIAG_STEPS.map((id) => {
              const row = results?.find((r) => r.id === id),
                state = row
                  ? row.skipped
                    ? "diag.skipped"
                    : row.ok
                      ? "diag.passed"
                      : "diag.failed"
                  : checking === id
                    ? "diag.checking"
                    : runFailed ? "diag.incomplete" : "diag.pending";
              const isFailedRow = !!row && !row.ok && !row.skipped;
              const isFirstFailure = canRepair && isFailedRow && row.id === first?.id;
              return (
                <section
                  key={id}
                  id={`cb-check-${id}`}
                  tabIndex={-1}
                  className={`cb-card cb-diagnostic-step ${isFailedRow ? "has-issue" : ""}${isFirstFailure ? " first-failed" : ""}`}
                  aria-busy={running && checking === id}
                >
                  <span className="cb-check-symbol" data-state={!row?"pending":row.skipped?"skipped":row.ok?"passed":"failed"} aria-hidden="true"><Icon name={!row||row.skipped?"minus":row.ok?"check":"close"} size={16} strokeWidth={2}/></span>
                  <div>
                    <h3>
                      {t(`check.${id}`)}
                      {isFirstFailure && <span className="cb-prio-tag">{t("diag.prio")}</span>}
                    </h3>
                    {row?.detail && <p className="cb-selectable">{row.detail}</p>}
                    {!running && report && row && !row.ok && !row.skipped && (
                      <a className="cb-diagnostic-guide" href={guideUrl(id, currentLang(), report.context)}
                        onClick={event => {
                          event.preventDefault();
                          void ipc.openDiagnosisGuide(event.currentTarget.href)
                            .catch(error => notify.error(error, "diagnostics-guide"));
                        }}>
                        {t("diag.summaryGuide")}
                      </a>
                    )}
                  </div>
                  <span className={isFailedRow ? "cb-check-state-fail" : undefined}>{t(state)}</span>
                  {canRepair && isFailedRow && row?.fix && (
                    <Button
                      size="small"
                      loading={fixing?.id === id}
                      disabled={repairRunning}
                      onClick={() => void fix(row)}
                    >
                      {t(`diag.fix.${row.fix}`)}
                    </Button>
                  )}
                  {isFirstFailure && !row?.fix && id === "config_renders" && (
                    <Button size="small" onClick={goConfig}>{t("diag.openConfig")}</Button>
                  )}
                </section>
              );
            })}
          </div>
        </>
      )}
      {summaryText !== null && (
        <Dialog title={t("diag.summaryPreview")} onClose={() => { clear(); setSummaryOpen(false); }}>
          <pre className="cb-share-preview cb-selectable cb-mono">{summaryText}</pre>
          <div className="actions">
            <Button variant="primary" onClick={() => void copy(summaryText, () => summaryText)}>
              <span role="status">{t(copied === summaryText ? "workflow.copied" : "diag.summaryCopy")}</span>
            </Button>
          </div>
        </Dialog>
      )}
    </div>
  );
}
