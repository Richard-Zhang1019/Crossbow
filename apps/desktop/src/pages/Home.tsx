import { useCallback, useEffect, useRef, useState } from "react";
import {
  CORE_MODES,
  ipc,
  type CoreMode,
  type CoreStatus,
  type Profile,
  type SysProxyStatus,
} from "../ipc";
import { fmtBytes } from "../format";
import { useT } from "../i18n";

const POINTS = 120;

/** 首页仪表盘：系统代理总开关 + 模式 + 实时速率曲线 + 累计统计。 */
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
  const mounted = useRef(true);

  const refresh = useCallback(async () => {
    if (!mounted.current) return;
    setProfiles(await ipc.listProfiles().catch(() => []));
    setActiveId(await ipc.activeProfileId().catch(() => null));
    setCore(await ipc.coreStatus().catch(() => ({ state: "Stopped" as const })));
    setProxy(await ipc.sysproxyStatus().catch(() => ({ enabled: false, port: 0 })));
    setMode(await ipc.coreMode().catch(() => "rule" as const));
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

  return (
    <div className="mx-auto max-w-4xl space-y-5">
      <h1 className="text-lg font-semibold">{t("nav.home")}</h1>

      <section className="rounded-xl border p-5"
        style={{ background: "var(--cb-surface)", borderColor: "var(--cb-border)" }}>
        <div className="flex items-center justify-between">
          <div>
            <div className="text-sm font-medium">
              {t("home.sysproxy")}
              {proxy.enabled ? ` :${proxy.port}` : ""}
            </div>
            <div className="mt-0.5 text-xs" style={{ color: "var(--cb-text-dim)" }}>
              {t("home.core")} {coreStatusText(t, core)} · {t("home.activeProfile")}{" "}
              {active ? active.name : t("home.none")}            </div>
          </div>
          <button
            onClick={toggleProxy}
            disabled={busy || !activeId}
            title={activeId ? undefined : t("home.noProfiles")}
            className="rounded-full px-4 py-1.5 text-sm font-medium text-white disabled:opacity-40"
            style={{ background: proxy.enabled ? "var(--cb-accent)" : "#8e8e93" }}
          >
            {busy ? "…" : proxy.enabled ? t("common.off") : t("common.on")}
          </button>
        </div>

        <div className="mt-4 flex items-center gap-1">
          {CORE_MODES.map((m) => (
            <button
              key={m.id}
              onClick={() => ipc.setCoreMode(m.id).catch((e) => setError(String(e)))}
              className="rounded-lg px-3 py-1 text-xs"
              style={
                mode === m.id
                  ? { background: "var(--cb-accent)", color: "#fff" }
                  : { color: "var(--cb-text-dim)", border: "1px solid var(--cb-border)" }
              }
            >
              {t(`mode.${m.id}` as const)}
            </button>
          ))}
          <span className="ml-2 text-[11px]" style={{ color: "var(--cb-text-dim)" }}>
            {t("home.modeHint")}
          </span>
        </div>
      </section>

      {error && (
        <div className="rounded-lg border px-3 py-2 text-xs"
          style={{ borderColor: "#e0524c", color: "#e0524c" }}>
          {error}
        </div>
      )}

      <div className="grid grid-cols-3 gap-4">
        <Card title={t("home.outbound")}>
          <div className="text-sm">— {t("home.comingM1")} —</div>
        </Card>
        <Card title={t("home.traffic")}>
          <div className="text-sm cb-selectable">
            ↑ {fmtBytes(totals.up)} · ↓ {fmtBytes(totals.down)}
          </div>
        </Card>
        <Card title={t("home.connections")}>
          <div className="text-sm">{connCount ?? "—"}</div>
        </Card>
      </div>

      <Card title={`${t("home.rate")}　↑ ${fmtBytes(latest?.up ?? 0)}/s · ↓ ${fmtBytes(latest?.down ?? 0)}/s`}>
        <Sparkline rates={rates} />
      </Card>

      <Card title={t("home.profiles")}>
        {profiles.length === 0 ? (
          <div className="text-xs" style={{ color: "var(--cb-text-dim)" }}>
            {t("home.noProfiles")}
          </div>
        ) : (
          <ul className="space-y-1 text-sm">
            {profiles.map((p) => (
              <li key={p.id} className="cb-selectable">
                {p.id === activeId ? "● " : "○ "}
                {p.kind === "remote" ? "☁" : "☰"} {p.name}
              </li>
            ))}
          </ul>
        )}
      </Card>
    </div>
  );
}

function coreStatusText(
  t: ReturnType<typeof useT>,
  s: CoreStatus,
): string {
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

function Sparkline({ rates }: { rates: { up: number; down: number }[] }) {
  const W = 860;
  const H = 96;
  const max = Math.max(1, ...rates.map((r) => Math.max(r.up, r.down)));
  const path = (key: "up" | "down") => {
    if (rates.length < 2) return "";
    const step = W / (POINTS - 1);
    const offset = POINTS - rates.length;
    return rates
      .map((r, i) => {
        const x = (i + offset) * step;
        const y = H - (r[key] / max) * (H - 6) - 3;
        return `${i === 0 ? "M" : "L"}${x.toFixed(1)},${y.toFixed(1)}`;
      })
      .join(" ");
  };
  return (
    <svg viewBox={`0 0 ${W} ${H}`} className="w-full" style={{ height: 96 }}>
      <path d={path("down")} fill="none" stroke="#4a9df8" strokeWidth="1.5" />
      <path d={path("up")} fill="none" stroke="var(--cb-accent)" strokeWidth="1.5" />
    </svg>
  );
}

function Card({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <section className="rounded-xl border p-4"
      style={{ background: "var(--cb-surface)", borderColor: "var(--cb-border)" }}>
      <div className="mb-2 text-xs font-medium" style={{ color: "var(--cb-text-dim)" }}>
        {title}
      </div>
      {children}
    </section>
  );
}
