import { useState } from "react";
import { ipc, type CheckResult } from "../ipc";
import { useT } from "../i18n";
import { PageHead } from "./Home";
import { Icon } from "../components/Icon";

/** 网络诊断页（M1.2）：一键自检 + 可执行修复动作。 */
export default function DiagnosticsPage() {
  const t = useT();
  const [running, setRunning] = useState(false);
  const [results, setResults] = useState<CheckResult[] | null>(null);
  const [fixing, setFixing] = useState<string | null>(null);

  const run = async () => {
    setRunning(true);
    try {
      setResults(await ipc.runDiagnosis());
    } catch (e) {
      console.error(e);
    } finally {
      setRunning(false);
    }
  };

  const applyFix = async (check: CheckResult) => {
    if (!check.fix) return;
    setFixing(check.id);
    try {
      if (check.fix === "start_core") await ipc.coreStart();
      if (check.fix === "restart_core") {
        await ipc.coreStop().catch(() => {});
        await new Promise((r) => setTimeout(r, 500));
        await ipc.coreStart();
      }
      if (check.fix === "toggle_sysproxy") await ipc.sysproxyToggle();
    } catch (e) {
      console.error(e);
    } finally {
      setFixing(null);
      void run(); // 修复后自动重跑
    }
  };

  const allOk = results?.every((r) => r.ok);

  return (
    <div className="mx-auto max-w-4xl space-y-3.5">
      <PageHead title={t("diag.title")} meta={t("diag.hint")}>
        <button className="cb-btn acc" onClick={run} disabled={running}>
          {running ? t("diag.running") : t("diag.run")}
        </button>
      </PageHead>

      {results === null ? (
        <div
          className="cb-card flex h-56 flex-col items-center justify-center gap-2 text-sm"
          style={{ color: "var(--cb-text-dim)" }}
        >
          <Icon name="pulse" size={20} stroke="var(--cb-faint)" />
          <span>{t("diag.hint")}</span>
        </div>
      ) : (
        <div className="space-y-2">
          {results.map((r) => (
            <section key={r.id} className="cb-card flex items-center gap-3 px-4 py-3">
              <i
                className="h-[7px] w-[7px] flex-none rounded-full"
                style={{
                  background: r.ok ? "var(--cb-ok)" : "var(--cb-bad)",
                  boxShadow: r.ok
                    ? "0 0 6px color-mix(in srgb, var(--cb-ok) 55%, transparent)"
                    : "0 0 6px color-mix(in srgb, var(--cb-bad) 55%, transparent)",
                }}
              />
              <div className="min-w-0 flex-1">
                <div className="text-[13px] font-medium">
                  {t(`check.${r.id}` as const)}
                </div>
                {!r.ok && (
                  <div
                    className="cb-selectable mt-0.5 text-[11.5px]"
                    style={{ color: "var(--cb-text-dim)" }}
                  >
                    {r.detail}
                  </div>
                )}
              </div>
              {r.ok ? (
                <span className="cb-mono text-[11px]" style={{ color: "var(--cb-text-dim)" }}>
                  OK
                </span>
              ) : r.fix ? (
                <button
                  className="cb-btn acc py-1.5"
                  disabled={fixing !== null}
                  onClick={() => applyFix(r)}
                >
                  {fixing === r.id ? "…" : t(`diag.fix.${r.fix}` as const)}
                </button>
              ) : null}
            </section>
          ))}

          {allOk && (
            <div
              className="rounded-lg border px-3 py-2 text-xs"
              style={{
                borderColor: "var(--cb-accent-line)",
                color: "var(--cb-accent)",
                background: "var(--cb-accent-soft)",
              }}
            >
              {t("diag.allOk")}
            </div>
          )}
        </div>
      )}
    </div>
  );
}
