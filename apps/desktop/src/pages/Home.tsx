import { useCallback, useEffect, useRef, useState } from "react";
import {
  CORE_MODES,
  ipc,
  type CoreMode,
  type CoreStatus,
  type OutboundInfo,
  type Profile,
  type SysProxyStatus,
} from "../ipc";
import { fmtBytes } from "../format";
import { useT } from "../i18n";
import { Icon } from "../components/Icon";

const POINTS = 120;

/** 首页（v2）：状态卡 + KPI + 平滑速率曲线 + 配置档案。 */
export default function HomePage() {
  const t = useT();
  const [profiles, setProfiles] = useState<Profile[]>([]);
  const [activeId, setActiveId] = useState<string | null>(null);
  const [core, setCore] = useState<CoreStatus>({ state: "Stopped" });
  const [proxy, setProxy] = useState<SysProxyStatus>({ enabled: false, port: 0 });
  const [mode, setMode] = useState<CoreMode>("rule");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [rates, setRates] = useState<{ up: number; down: number }[]>([]);
  const [totals, setTotals] = useState({ up: 0, down: 0 });
  const [connCount, setConnCount] = useState<number | null>(null);
  const [outbound, setOutbound] = useState<OutboundInfo | null>(null);
  const mounted = useRef(true);

  const refresh = useCallback(async () => {
    if (!mounted.current) return;
    setProfiles(await ipc.listProfiles().catch(() => []));
    setActiveId(await ipc.activeProfileId().catch(() => null));
    setCore(await ipc.coreStatus().catch(() => ({ state: "Stopped" as const })));
    setProxy(await ipc.sysproxyStatus().catch(() => ({ enabled: false, port: 0 })));
    setMode(await ipc.coreMode().catch(() => "rule" as const));
    setOutbound(await ipc.currentOutbound().catch(() => null));
  }, []);

  useEffect(() => {
    mounted.current = true;
    refresh();
    const un1 = ipc.onCoreStatus(setCore);
    const un2 = ipc.onSysproxyStatus(setProxy);
    const un3 = ipc.onCoreMode(setMode);
    ipc.subscribeTraffic().catch(() => {});
    const un4 = ipc.onTraffic(({ up, down }) => {
      if (!mounted.current) return;
      setRates((prev) => [...prev.slice(-(POINTS - 1)), { up, down }]);
      setTotals((t0) => ({ up: t0.up + up, down: t0.down + down }));
    });
    ipc.subscribeConnections().catch(() => {});
    const un5 = ipc.onConnections((s) => {
      if (mounted.current) setConnCount(s.rows.length);
    });
    return () => {
      mounted.current = false;
      for (const un of [un1, un2, un3, un4, un5]) void un.then((f) => f());
      ipc.unsubscribeTraffic().catch(() => {});
      ipc.unsubscribeConnections().catch(() => {});
    };
  }, [refresh]);

  const toggleProxy = async () => {
    setBusy(true);
    setError(null);
    try {
      await ipc.sysproxyToggle();
      await refresh();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const active = profiles.find((p) => p.id === activeId);
  const latest = rates[rates.length - 1];
  const running = core.state === "Running" || core.state === "Starting";

  return (
    <div className="mx-auto max-w-4xl space-y-3.5">
      <PageHead title={t("nav.home")} meta={`mixed :${proxy.port || 7897} · ${mode}`} />

      {error && <ErrorBar msg={error} />}

      {/* 状态卡 */}
      <section className="cb-card flex items-center gap-4 px-5 py-[18px]">
        <span
          className={`cb-dot ${
            core.state === "Crashed" || (proxy.enabled && !running)
              ? "bad"
              : proxy.enabled && running
                ? "ok"
                : "idle"
          }`}
        />
        <div className="flex-1">
          <div className="text-[14px] font-semibold">
            {proxy.enabled ? t("home.sysproxyOn") : t("home.sysproxyOff")}
          </div>
          <div className="mt-0.5 text-[11.5px]" style={{ color: "var(--cb-text-dim)" }}>
            {coreText(t, core)} · {t("home.activeProfile")}{" "}
            {active ? active.name : t("home.none")}
          </div>
        </div>
        <div className="cb-seg">
          {CORE_MODES.map((m) => (
            <button
              key={m.id}
              className={mode === m.id ? "on" : ""}
              onClick={() => ipc.setCoreMode(m.id).catch((e) => setError(String(e)))}
            >
              {t(`mode.${m.id}` as const)}
            </button>
          ))}
        </div>
        <button
          className={`cb-toggle ${proxy.enabled ? "on" : ""}`}
          onClick={toggleProxy}
          disabled={busy || !activeId}
          title={activeId ? undefined : t("home.noProfiles")}
        />
        <div className="h-10 w-px" style={{ background: "var(--cb-line)" }} />
        <div className="min-w-[170px] text-right">
          <div className="cb-micro">{t("home.outbound")}</div>
          {outbound ? (
            <>
              <div className="cb-mono mt-0.5 text-[13.5px] font-semibold">
                {outbound.node}
                {outbound.delay_ms != null && (
                  <span
                    className="ml-1.5 text-[11px] font-normal"
                    style={{
                      color:
                        outbound.delay_ms <= 200
                          ? "var(--cb-ok)"
                          : outbound.delay_ms <= 500
                            ? "var(--cb-warn)"
                            : "var(--cb-bad)",
                    }}
                  >
                    {outbound.delay_ms}ms
                  </span>
                )}
              </div>
              <div
                className="cb-mono mt-0.5 truncate text-[11px]"
                style={{ color: "var(--cb-faint)" }}
                title={outbound.chain.join(" → ")}
              >
                {outbound.chain.join(" → ")} · {outbound.mode}
              </div>
            </>
          ) : (
            <div
              className="cb-mono mt-0.5 text-[11px]"
              style={{ color: "var(--cb-faint)" }}
            >
              内核未运行
            </div>
          )}
        </div>
      </section>

      {/* KPI */}
      <div className="grid grid-cols-3 gap-3">
        <Kpi
          micro="↑ 上行速率"
          value={fmtBytes(latest?.up ?? 0)}
          delta={`累计 ${fmtBytes(totals.up)}`}
        />
        <Kpi
          micro="↓ 下行速率"
          value={fmtBytes(latest?.down ?? 0)}
          delta={`累计 ${fmtBytes(totals.down)}`}
        />
        <Kpi micro={t("home.connections")} value={connCount != null ? String(connCount) : "—"} delta="" />
      </div>

      {/* 速率曲线 */}
      <section className="cb-card grow px-4 pb-2 pt-4">
        <div className="flex items-baseline">
          <span className="cb-micro flex-1">{t("home.rate")}</span>
          <span className="cb-mono text-[12px]" style={{ color: "var(--cb-up)" }}>
            ↑ {fmtBytes(latest?.up ?? 0)}/s
          </span>
          <span className="cb-mono ml-3 text-[12px]" style={{ color: "var(--cb-down)" }}>
            ↓ {fmtBytes(latest?.down ?? 0)}/s
          </span>
        </div>
        <Sparkline rates={rates} />
      </section>

      {/* 配置档案 */}
      <section className="cb-card px-4 pb-4 pt-[14px]">
        <div className="cb-micro">{t("home.profiles")}</div>
        <div className="mt-2.5 flex flex-col gap-2">
          {profiles.length === 0 ? (
            <div className="text-xs" style={{ color: "var(--cb-text-dim)" }}>
              {t("home.noProfiles")}
            </div>
          ) : (
            profiles.map((p) => {
              const on = p.id === activeId;
              const tr = p.traffic;
              const used = tr ? tr.upload + tr.download : 0;
              const pct = tr && tr.total > 0 ? Math.min(100, (used / tr.total) * 100) : 0;
              return (
                <div
                  key={p.id}
                  className="flex items-center gap-3 rounded-[10px] px-3.5 py-2.5"
                  style={{
                    border: `1px solid ${on ? "var(--cb-accent-line)" : "var(--cb-line)"}`,
                    background: on
                      ? "linear-gradient(180deg, var(--cb-accent-soft), transparent)"
                      : "var(--cb-surface-2)",
                  }}
                >
                  <span className={on ? "cb-badge" : "cb-badge gray"}>
                    {on ? "当前" : p.kind === "remote" ? "订阅" : "本地"}
                  </span>
                  <b className="text-[12.5px] font-medium">{p.name}</b>
                  {tr && tr.total > 0 && (
                    <>
                      <div
                        className="h-[3px] flex-1 overflow-hidden rounded-full"
                        style={{ background: "color-mix(in srgb, var(--cb-text) 7%, transparent)" }}
                      >
                        <div
                          className="h-full rounded-full"
                          style={{
                            width: `${pct}%`,
                            background: "linear-gradient(90deg, var(--cb-accent), var(--cb-down))",
                          }}
                        />
                      </div>
                      <span className="cb-mono text-[11px]" style={{ color: "var(--cb-faint)" }}>
                        {fmtBytes(used)} / {fmtBytes(tr.total)}
                      </span>
                    </>
                  )}
                </div>
              );
            })
          )}
        </div>
      </section>
    </div>
  );
}

export function PageHead({ title, meta, children }: { title: string; meta?: string; children?: React.ReactNode }) {
  return (
    <div className="mb-1 flex items-center gap-3">
      <h1 className="text-[15px] font-semibold tracking-tight">{title}</h1>
      {meta && (
        <span className="cb-mono text-[11px]" style={{ color: "var(--cb-faint)" }}>
          {meta}
        </span>
      )}
      <span className="flex-1" />
      {children}
    </div>
  );
}

export function ErrorBar({ msg }: { msg: string }) {
  return (
    <div
      className="rounded-lg border px-3 py-2 text-xs"
      style={{ borderColor: "color-mix(in srgb, var(--cb-bad) 40%, transparent)", color: "var(--cb-bad)" }}
    >
      {msg}
    </div>
  );
}

function Kpi({ micro, value, delta }: { micro: string; value: string; delta: string }) {
  return (
    <section className="cb-card px-4 py-3.5">
      <div className="cb-micro">{micro}</div>
      <div className="cb-mono mt-1 text-[21px] font-semibold tracking-tight">
        {value}
      </div>
      {delta && (
        <div className="cb-mono mt-1 text-[10.5px]" style={{ color: "var(--cb-faint)" }}>
          {delta}
        </div>
      )}
    </section>
  );
}

/** 平滑贝塞尔双线 sparkline（对齐设计稿 v2）。 */
function Sparkline({ rates }: { rates: { up: number; down: number }[] }) {
  const W = 640;
  const H = 120;
  const max = Math.max(1, ...rates.map((r) => Math.max(r.up, r.down)));
  const smooth = (key: "up" | "down") => {
    if (rates.length < 2) return "";
    const step = W / (POINTS - 1);
    const offset = POINTS - rates.length;
    const pts = rates.map((r, i) => ({
      x: (i + offset) * step,
      y: H - (r[key] / max) * (H - 10) - 4,
    }));
    let d = `M${pts[0].x.toFixed(1)},${pts[0].y.toFixed(1)}`;
    for (let i = 1; i < pts.length; i++) {
      const p0 = pts[i - 1];
      const p1 = pts[i];
      const mx = ((p0.x + p1.x) / 2).toFixed(1);
      d += ` C${mx},${p0.y.toFixed(1)} ${mx},${p1.y.toFixed(1)} ${p1.x.toFixed(1)},${p1.y.toFixed(1)}`;
    }
    return d;
  };
  const upLine = smooth("up");
  return (
    <svg viewBox={`0 0 ${W} ${H}`} preserveAspectRatio="none" className="mt-2 w-full" style={{ height: 120 }}>
      <defs>
        <linearGradient id="au" x1="0" y1="0" x2="0" y2="1">
          <stop offset="0" style={{ stopColor: "var(--cb-up)", stopOpacity: 0.16 }} />
          <stop offset="1" style={{ stopColor: "var(--cb-up)", stopOpacity: 0 }} />
        </linearGradient>
      </defs>
      <g style={{ stroke: "var(--cb-line)" }}>
        <line x1="0" y1="30" x2={W} y2="30" />
        <line x1="0" y1="60" x2={W} y2="60" />
        <line x1="0" y1="90" x2={W} y2="90" />
      </g>
      {upLine && (
        <>
          <path d={`${upLine} L${W},${H} L0,${H} Z`} fill="url(#au)" />
          <path d={upLine} fill="none" style={{ stroke: "var(--cb-up)" }} strokeWidth="1.6" />
        </>
      )}
      {smooth("down") && (
        <path d={smooth("down")} fill="none" style={{ stroke: "var(--cb-down)" }} strokeWidth="1.4" opacity=".9" />
      )}
    </svg>
  );
}

function coreText(t: ReturnType<typeof useT>, s: CoreStatus): string {
  switch (s.state) {
    case "Running":
      return t("home.coreRunning");
    case "Starting":
      return t("home.coreStarting");
    case "Crashed":
      return t("home.coreCrashed", { msg: s.message ?? "?" });
    default:
      return t("home.coreStopped");
  }
}

// Icon 供 KPI 卡未来扩展使用，避免未用告警
void Icon;
