import Dialog from "@/components/Dialog";
import { useClipboardFeedback } from "@/hooks/useClipboardFeedback";
import { Icon } from "@/components/Icon";
import { Button } from "@/components/Button";
import { notify, notificationText } from "@/notifications/service";
import { useOperationNotice } from "@/notifications/NotificationProvider";
import { useEffect, useRef, useState, type MutableRefObject } from "react";
import { ipc, type CheckResult, type FaultEvidence, type FaultEvidenceProbe } from "@/services/ipc";
import { currentLang, useT } from "@/i18n/i18n";
import { PageHead } from "@/pages/Home";
import { ProgressRing } from "@/components/ProgressRing";
import { DIAG_STEPS, navigateTo } from "@/lib/workflow";
import { failedChecks, guideUrl, isStale, remainingAge, shareText, type DiagnosisSnapshot } from "./report";

let previousReport: DiagnosisSnapshot | null = null;

/** http_status 类别附带状态码；其余类别无码可示。 */
const fmtStatus = (probe: FaultEvidenceProbe) =>
  probe.class === "http_status" && probe.httpStatus != null ? ` ${probe.httpStatus}` : "";

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
  const [fault, setFault] = useState<FaultEvidence | null>(null);
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

  // #488：最近一次系统代理故障取证（观察模式，可能为空）；焦点时刷新。
  useEffect(() => {
    let cancelled = false;
    const load = () => {
      ipc.sysproxyFaultEvidence()
        .then(evidence => { if (!cancelled) setFault(evidence); })
        .catch(() => {});
    };
    load();
    window.addEventListener("focus", load);
    return () => {
      cancelled = true;
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
  const summaryText = summaryOpen && report ? shareText(report, appVersion, stale || runFailed) : null;
  const resultTitle = running
    ? t("diag.progress", { done: progress.length, total: DIAG_STEPS.length })
    : runFailed ? t("diag.incomplete")
    : stale ? t("diag.stale")
    : first ? t("diag.resultIssues", { n: failed.length })
    : applicable ? t("diag.applicableOk") : t("diag.title");
  const goConfig = () => window.dispatchEvent(new CustomEvent("cb:goto", { detail: "profiles" }));
  return (
    <div className="cb-diagnostics-page">
      <PageHead title={t("diag.title")} meta={t("diag.hint")}>
        {report && !running && (
          <Button onClick={() => setSummaryOpen(true)}>
            {t("diag.summaryPreview")}
          </Button>
        )}
        <Button variant="primary" disabled={!listeners} loading={running || repairRunning} onClick={() => void run()}>
          <span role="status">{running ? t("diag.running") : repairRunning ? repairLabel : t(stale || runFailed ? "diag.rerun" : "diag.run")}</span>
        </Button>
      </PageHead>
      {fault && <section className="cb-card cb-fault-evidence" aria-label={t("fault.title")}>
        <div className="cb-fault-evidence-head">
          <h2>{t("fault.title")}</h2>
          <span className={fault.resolvedAtMs ? "cb-fault-state" : "cb-fault-state ongoing"} role="status">
            {t(fault.resolvedAtMs ? "fault.resolved" : "fault.ongoing")}
          </span>
        </div>
        <p className="cb-fault-time">
          {t("fault.startedAt")}：{new Date(fault.startedAtMs).toLocaleString(currentLang() === "en" ? "en-US" : "zh-CN")}
          {fault.resolvedAtMs != null && <> · {t("fault.resolvedAt")}：{new Date(fault.resolvedAtMs).toLocaleString(currentLang() === "en" ? "en-US" : "zh-CN")}</>}
        </p>
        <dl className="cb-fault-layers">
          <div><dt>{t("fault.mixedPort")}</dt><dd>{fault.mixedPortOk ? t("fault.class.ok") : t("fault.class.connect")}</dd></div>
          <div><dt>{t("fault.controller")}</dt><dd>{t(`fault.class.${fault.controller.class}`)}{fmtStatus(fault.controller)}</dd></div>
          <div><dt>{t("fault.directCn")}</dt><dd>{t(`fault.class.${fault.directCn.class}`)}{fmtStatus(fault.directCn)}</dd></div>
          <div><dt>{t("fault.directGlobal")}</dt><dd>{t(`fault.class.${fault.directGlobal.class}`)}{fmtStatus(fault.directGlobal)}</dd></div>
          <div><dt>{t("fault.proxyHttps")}</dt><dd>{t(`fault.class.${fault.proxyHttps.class}`)}{fmtStatus(fault.proxyHttps)}</dd></div>
        </dl>
        {fault.sysproxyObserved.length > 0 && (
          <p className="cb-fault-observed cb-selectable">
            {t("fault.observed")}：{fault.sysproxyObserved.join("；")}
          </p>
        )}
      </section>}
      {!results && !runFailed ? (
        <div className="cb-empty">
          <p>{repairRunning ? repairLabel : t("diag.hint")}</p>
        </div>
      ) : (
        <>
          <div className="cb-diag-summary" aria-live="polite">
            {running ? (
              <ProgressRing value={progress.length / DIAG_STEPS.length * 100} label={resultTitle} size={28} />
            ) : (
              <span className="cb-diagnostic-result" data-result={canRepair && applicable && !first ? "passed" : "attention"} aria-hidden="true">
                <Icon name={canRepair && applicable && !first ? "check" : "activity"} size={22} />
              </span>
            )}
            <div>
              <h2>{resultTitle}</h2>
              {!running && report && <p>{t("diag.checkedAt", { time: new Date(report.completedAt).toLocaleString() })}</p>}
            </div>
          </div>
          {canRepair && first && (
            <section
              id="cb-check-first-problem"
              tabIndex={-1}
              className="cb-workflow-callout cb-diag-blocker"
            >
              <div>
                <span>{t("diag.firstBlocker")}</span>
                <h3>{t(`check.${first.id}`)}</h3>
                <p>{t("diag.nextStep")}</p>
              </div>
              {first.fix ? (
                <Button
                  loading={fixing?.id === first.id}
                  disabled={repairRunning}
                  onClick={() => void fix(first)}
                >
                  {t(`diag.fix.${first.fix}`)}
                </Button>
              ) : (
                <Button
                  onClick={
                    first.id === "config_renders"
                      ? goConfig
                      : () => document.getElementById(`cb-check-${first.id}`)?.focus()
                  }
                >
                  {first.id === "config_renders" ? t("diag.openConfig") : t("workflow.details")}
                </Button>
              )}
            </section>
          )}
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
              return (
                <section
                  key={id}
                  id={`cb-check-${id}`}
                  tabIndex={-1}
                  className={`cb-card cb-diagnostic-step ${row && !row.ok && !row.skipped ? "has-issue" : ""}`}
                  aria-busy={running && checking === id}
                >
                  <span className="cb-check-symbol" data-state={!row?"pending":row.skipped?"skipped":row.ok?"passed":"failed"} aria-hidden="true"><Icon name={!row||row.skipped?"minus":row.ok?"check":"close"} size={16} strokeWidth={2}/></span>
                  <div>
                    <h3>{t(`check.${id}`)}</h3>
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
                  <span>{t(state)}</span>
                  {canRepair && row?.fix && (
                    <Button
                      size="small"
                      loading={fixing?.id === id}
                      disabled={repairRunning}
                      onClick={() => void fix(row)}
                    >
                      {t(`diag.fix.${row.fix}`)}
                    </Button>
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
