import {Icon} from "@/components/Icon";
import { Button } from "@/components/Button";
import { notify, notificationText } from "@/notifications/service";
import { useOperationNotice } from "@/notifications/NotificationProvider";
import { useEffect, useRef, useState } from "react";
import { ipc, type CheckResult } from "@/services/ipc";
import { useT } from "@/i18n/i18n";
import { PageHead } from "@/pages/Home";
import { ProgressRing } from "@/components/ProgressRing";
import { DIAG_STEPS } from "@/lib/workflow";
// #469：结果快照带完成时间——接管方式/系统代理/端口等变化后判定过期，
// 只显示「上次检查时间」并提供重新检查入口，不再把旧结论当当前状态。
let previousResults: CheckResult[] | null = null;
let previousCompletedAt: number | null = null;
const STALE_AFTER_MS = 60_000;
export default function DiagnosticsPage() {
  const t = useT();
  const [running, setRunning] = useState(false);
  const [results, setResults] = useState<CheckResult[] | null>(previousResults);
  const [completedAt, setCompletedAt] = useState<number | null>(previousCompletedAt);
  const [stale, setStale] = useState(false);
  const [runFailed,setRunFailed]=useState(false);
  const [checking, setChecking] = useState<string | null>(null);
  const [fixing, setFixing] = useState<CheckResult | null>(null);
  const restartRunning = useOperationNotice("core-restart")?.kind === "progress";
  const repairRunning = !!fixing || restartRunning;
  const repairLabel = restartRunning || fixing?.fix === "restart_core"
    ? notificationText("core.restarting")
    : fixing?.fix === "start_core" ? t("diag.startingCore") : t("diag.switchingProxy");
  const fixingRef = useRef(false);
  const coreRevision = useRef(0);
  const active = useRef(true),
    runId = useRef<string | null>(null),
    inFlight = useRef(false);
  useEffect(() => {
    active.current = true;
    const invalidate = () => {
      coreRevision.current += 1;
      previousResults = null;
      if (active.current) { setResults(null); setCompletedAt(null); }
    };
    const off = ipc.onCoreStatus(invalidate);
    // #469：接管方式与系统代理变化（内核可能持续 Running）同样使旧结论过期
    const offProxy = ipc.onSysproxyStatus(() => invalidate());
    const offSwitched = ipc.onProfileSwitched(() => invalidate());
    // Cached reports can also become stale while this page is unmounted.
    const revision = coreRevision.current;
    void off.then(() => ipc.coreStatus()).then(status => {
      if (!active.current || coreRevision.current !== revision) return;
      const core = previousResults?.find(row => row.id === "core_running");
      if (core && core.ok !== (status.state === "Running")) invalidate();
    }).catch(() => {});
    // 挂载时若已有结果且超过过期窗口，标记为过期而非当作当前结论
    setStale(previousCompletedAt !== null && Date.now() - previousCompletedAt > STALE_AFTER_MS);
    void offProxy.then(unsubscribe => unsubscribe()).catch(() => {});
    void offSwitched.then(unsubscribe => unsubscribe()).catch(() => {});
    return () => {
      active.current = false;
      runId.current = null;
      void off.then(unsubscribe => unsubscribe()).catch(() => {});
    };
  }, []);
  const run = async () => {
    if (!active.current || inFlight.current) return;
    inFlight.current = true;
    setRunning(true);
    setRunFailed(false);
    setResults([]);
    const id = crypto.randomUUID();
    const revision = coreRevision.current;
    runId.current = id;
    let off: (() => void) | undefined;
    try {
      off = await ipc.onDiagnosisProgress((p) => {
        if (!active.current || p.run_id !== runId.current || coreRevision.current !== revision) return;
        if (!p.result) setChecking(p.id);
        else setResults((old) => [...(old ?? []).filter((r) => r.id !== p.id), p.result!]);
      });
      if (!active.current) return;
      const rows = await ipc.runDiagnosis(id);
      if (!active.current || coreRevision.current !== revision) return;
      previousResults = rows;
      previousCompletedAt = Date.now();
      if (active.current && runId.current === id) {
        setResults(rows);
        void notify.resolve("diagnostics-operation");
      }
    } catch (e) {
      if (active.current) {setRunFailed(true);notify.error(e, "diagnostics-operation");}
    } finally {
      off?.();
      inFlight.current = false;
      if (active.current) {
        setChecking(null);
        setRunning(false);
      }
    }
  };
  const fix = async (r: CheckResult) => {
    if (!r.fix || fixingRef.current || inFlight.current || restartRunning) return;
    fixingRef.current = true;
    setFixing(r);
    try {
      if (r.fix === "start_core") await ipc.coreStart();
      if (r.fix === "restart_core") await ipc.coreRestart();
      if (r.fix === "toggle_sysproxy") await ipc.sysproxyToggle();
      void notify.resolve("diagnostics-fix");
      await run();
    } catch (e) {
      if (r.fix === "start_core") notify.error(e, "diagnostics-fix");
    } finally {
      fixingRef.current = false;
      if (active.current) setFixing(null);
    }
  };
  const first = results?.find((r) => !r.ok && !r.skipped),
    applicable = results?.filter((r) => !r.skipped) ?? [];
  const goConfig = () => window.dispatchEvent(new CustomEvent("cb:goto", { detail: "profiles" }));
  return (
    <div className="cb-diagnostics-page">
      <PageHead title={t("diag.title")} meta={t("diag.hint")}>
        <Button
          variant="primary"
          loading={running || repairRunning}
          onClick={() => void run()}
        >
          <span role="status">{running ? t("diag.running") : repairRunning ? repairLabel : t("diag.run")}</span>
        </Button>
      </PageHead>
      {results === null ? (
        <div className="cb-empty">
          <p>{repairRunning ? repairLabel : t("diag.hint")}</p>
        </div>
      ) : (
        <>
          <div className="cb-diag-summary" aria-live="polite">
            {!running && !runFailed && results.length === DIAG_STEPS.length && applicable.length > 0 && !first ? <span className="cb-diagnostic-result" data-result="passed" aria-hidden="true"><Icon name="check" size={22} strokeWidth={2}/></span> : running || runFailed ? <ProgressRing
              value={(results.length / DIAG_STEPS.length) * 100}
              label={t("diag.progress", { done: results.length, total: DIAG_STEPS.length })}
              size={28}
            /> : <span className="cb-diagnostic-result" data-result="attention" aria-hidden="true"><Icon name="activity" size={22}/></span>}
            <h2>
              {running
                ? t("diag.progress", { done: results.length, total: DIAG_STEPS.length })
                : first
                  ? t("diag.resultIssues", { n: results.filter((r) => !r.ok && !r.skipped).length })
                  : t(runFailed?"diag.incomplete":applicable.length ? "diag.applicableOk" : "diag.title")}
            </h2>
          </div>
          {!running && stale && results?.length ? (
            <div className="cb-diag-stale" role="status">
              {t("diag.stale")} · {completedAt ? new Date(completedAt).toLocaleTimeString() : ""}
              <Button onClick={run}>{t("diag.rerun")}</Button>
            </div>
          ) : null}
          {!running && first && (
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
              const row = results.find((r) => r.id === id),
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
                  </div>
                  <span>{t(state)}</span>
                  {!running && row?.fix && (
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
    </div>
  );
}
