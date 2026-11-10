import {Icon} from "@/components/Icon";
import { Button } from "@/components/Button";
import { notify } from "@/notifications/service";
import { useEffect, useRef, useState } from "react";
import { ipc, type CheckResult } from "@/services/ipc";
import { useT } from "@/i18n/i18n";
import { PageHead } from "@/pages/Home";
import { ProgressRing } from "@/components/ProgressRing";
import { DIAG_STEPS } from "@/lib/workflow";
let previousResults: CheckResult[] | null = null;
export default function DiagnosticsPage() {
  const t = useT();
  const [running, setRunning] = useState(false);
  const [results, setResults] = useState<CheckResult[] | null>(previousResults);
  const [runFailed,setRunFailed]=useState(false);
  const [checking, setChecking] = useState<string | null>(null);
  const [fixing, setFixing] = useState<string | null>(null);
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
      if (active.current) setResults(null);
    };
    const off = ipc.onCoreStatus(invalidate);
    // Cached reports can also become stale while this page is unmounted.
    const revision = coreRevision.current;
    void off.then(() => ipc.coreStatus()).then(status => {
      if (!active.current || coreRevision.current !== revision) return;
      const core = previousResults?.find(row => row.id === "core_running");
      if (core && core.ok !== (status.state === "Running")) invalidate();
    }).catch(() => {});
    return () => {
      active.current = false;
      runId.current = null;
      void off.then(unsubscribe => unsubscribe()).catch(() => {});
    };
  }, []);
  const run = async () => {
    if (inFlight.current) return;
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
    if (!r.fix || fixingRef.current || inFlight.current) return;
    fixingRef.current = true;
    setFixing(r.id);
    try {
      if (r.fix === "start_core") await ipc.coreStart();
      if (r.fix === "restart_core") {
        await ipc.coreStop();
        await ipc.coreStart();
      }
      if (r.fix === "toggle_sysproxy") await ipc.sysproxyToggle();
      void notify.resolve("diagnostics-fix");
      await run();
    } catch (e) {
      if (r.fix !== "toggle_sysproxy") notify.error(e, "diagnostics-fix");
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
          loading={running}
          disabled={running || !!fixing}
          onClick={() => void run()}
        >
          {t(running ? "diag.running" : "diag.run")}
        </Button>
      </PageHead>
      {results === null ? (
        <div className="cb-empty">
          <p>{t("diag.hint")}</p>
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
                  loading={fixing === first.id}
                  disabled={!!fixing}
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
                      loading={fixing === id}
                      disabled={!!fixing}
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
