import SegmentedControl from "@/components/SegmentedControl";
import { Switch } from "@/components/Controls";
import { HelpPopover } from "@/components/PopoverPanel";
import { Button } from "@/components/Button";
import NodeName from "@/components/NodeName";
import { nodeNameText } from "@/components/NodeName/nodeFlags";
import { notify, notificationText } from "@/notifications/service";
import { useCallback, useEffect, useRef, useState } from "react";
import {
  CORE_MODES,
  ipc,
  type CoreMode,
  type CoreStatus,
  type OutboundInfo,
  type Profile,
  type SysProxyStatus,
  type PlatformCapabilities,
} from "@/services/ipc";
import { fmtBytes } from "@/lib/format";
import { useT } from "@/i18n/i18n";
import { Icon } from "@/components/Icon";
import { ConnectionFlow } from "@/components/ConnectionFlow";

import { cumulativeTotals, createReadGate } from "@/lib/stateReliability";

const POINTS = 120;

/** 概览：连接状态、路径与趋势，配合独立的接管控制区。 */
export default function HomePage({ capabilities }: { capabilities: PlatformCapabilities | null }) {
  const t = useT();
  const explicitProxy = capabilities !== null && !capabilities.system_proxy;
  const [mixedPort, setMixedPort] = useState<number | null>(null);
  const [profiles, setProfiles] = useState<Profile[]>([]);
  const [activeId, setActiveId] = useState<string | null>(null);
  const [core, setCore] = useState<CoreStatus>({ state: "Stopped" });
  const [proxy, setProxy] = useState<SysProxyStatus>({
    enabled: false,
    port: 0,
  });
  const [tun, setTun] = useState({
    enabled: false,
    ready: false,
    coreRunning: false,
  });
  const [mode, setMode] = useState<CoreMode>("rule");
  const [busy, setBusy] = useState(false);
  const [rates, setRates] = useState<{ up: number; down: number }[]>([]);
  const [lastTrafficAt, setLastTrafficAt] = useState(0);
  const [totals, setTotals] = useState<{ up: number | null; down: number | null }>({ up: null, down: null });
  const [connCount, setConnCount] = useState<number | null>(null);
  const [outbound, setOutbound] = useState<OutboundInfo | null>(null);
  const mounted = useRef(true);
  const subscriptionGeneration = useRef(0);

  const readGate = useRef(createReadGate());
  const [known, setKnown] = useState<Set<string>>(new Set());
  const [readErrors, setReadErrors] = useState<Set<string>>(new Set());
  const confirm = useCallback((key: string) => {
    setKnown(old => new Set([...old, key]));
    setReadErrors(old => { const next = new Set(old); next.delete(key); return next; });
  }, []);
  const eventReceived = (key: string) => { readGate.current.invalidate(key); confirm(key); };

  // Conflict feedback shares the same queue and is deduplicated across polls.
  useEffect(() => {
    let active = true;
    let hadConflict = false;
    const check = async () => {
      try {
        const findings = await ipc.coexistDetect();
        if (!active) return;
        // 通知降噪（用户反馈）：只在其他 TUN 实际接管默认路由时弹——
        // 常驻服务/闲置内核进程属正常状态（退出 Verge 后其服务仍在），
        // 每次启动都提示是狼来了
        const tunnel = findings.find((finding) => finding.kind === "tunnel");
        if (tunnel) {
          hadConflict = true;
          notify.publish({
            kind: "warning",
            title: t("home.coexistTitle"),
            description: t("home.coexistHint"),
            details: findings.map((finding) => finding.detail).join("\n"),
            source: "coexist",
            operationId: "coexist",
            duration: 0,
            actions: ["diagnostics"],
          });
        } else if (hadConflict) {
          hadConflict = false;
          void notify.publish({
            kind: "success",
            code: "coexist.resolved",
            source: "coexist",
            operationId: "coexist",
          });
        }
      } catch (error) {
        if (active) notify.error(error, "coexist-read");
      }
    };
    void check();
    const timer = window.setInterval(check, 60_000);
    return () => {
      active = false;
      window.clearInterval(timer);
    };
  }, []);

  const refresh = useCallback(async () => {
    if (!mounted.current) return;
    const errors: { key: string; version: number }[] = [];
    const read = async <T,>(key: string, task: () => Promise<T>, commit: (value: T) => void) => {
      const version = readGate.current.begin(key);
      try {
        const value = await task();
        if (!mounted.current || !readGate.current.current(key, version)) return;
        commit(value); confirm(key);
        return value;
      } catch {
        if (!mounted.current || !readGate.current.current(key, version)) return;
        errors.push({ key, version });
        setReadErrors(old => new Set([...old, key]));
      }
    };
    const status = await read("core", ipc.coreStatus, setCore);
    if (status && status.state !== "Running") {
      readGate.current.invalidate("outbound");
      confirm("outbound");
    }
    await Promise.all([
      read("profiles", ipc.listProfiles, setProfiles),
      read("active", ipc.activeProfileId, setActiveId),
      read("proxy", ipc.sysproxyStatus, setProxy),
      read("tun", ipc.tunStatus, setTun),
      read("mode", ipc.coreMode, setMode),
      read("port", () => ipc.getEngineConfig().then(c => c.mixed_port), setMixedPort),
      status?.state === "Running" ? read("outbound", ipc.currentOutbound, setOutbound) : Promise.resolve(),
    ]);
    if (mounted.current && errors.some(({key, version}) => readGate.current.current(key, version))) notify.error(t("home.stateReadFailed"), "home-state-read");
    else if (mounted.current) void notify.resolve("home-state-read");
  }, [confirm]);

  useEffect(() => {
    const generation = ++subscriptionGeneration.current;
    let active = true;
    mounted.current = true;
    refresh();
    const un1 = ipc.onCoreStatus((status) => {
      if (!active) return;
      eventReceived("core"); setCore(status);
      if (status.state !== "Running") {
        setLastTrafficAt(0);
        // A controller is intentionally unavailable during stop/port handover.
        eventReceived("outbound");
      } else {
        void refresh();
      }
      const version = readGate.current.begin("tun");
      void ipc.tunStatus().then(value => {
        if (active && readGate.current.current("tun", version)) { setTun(value); confirm("tun"); }
      }).catch(e => {
        if (active && readGate.current.current("tun", version)) notify.error(e, "tun-state-read");
      });
    });
    const un2 = ipc.onSysproxyStatus(value => { if (active) { eventReceived("proxy"); setProxy(value); } });
    const untun = ipc.onTunStatus((enabled) => {
      if (active) { eventReceived("tun"); setTun((current) => ({ ...current, enabled })); }
    });
    const un3 = ipc.onCoreMode(value => { if (active) { eventReceived("mode"); setMode(value); } });
    const un4 = ipc.onTraffic(({ up, down }) => {
      if (!active || !mounted.current) return;
      if (
        (Number.isFinite(up) && up > 0) ||
        (Number.isFinite(down) && down > 0)
      ) {
        setLastTrafficAt(Date.now());
      }
      setRates((prev) => [...prev.slice(-(POINTS - 1)), { up, down }]);

    });
    const un5 = ipc.onConnections((s) => {
      if (active && mounted.current) {
        setConnCount(s.truncated ?? s.rows.length);
        setTotals(cumulativeTotals(s));
      }
    });

    // 速率历史预填（#181）：后端环形缓冲由托盘通道每秒喂入，与页面
    // 存亡无关——切页回来不再从空白重画
    void ipc
      .trafficHistory()
      .then((history) => {
        if (active && mounted.current && history.length > 0)
          setRates(history.slice(-POINTS));
      })
      .catch(() => {});

    // Install event listeners before starting the streams so that the first
    // websocket frames after a TUN/core restart cannot race past the webview.
    const subscribe = Promise.all([un1, un2, un3, un4, un5, untun]).then(() => {
      if (!active || subscriptionGeneration.current !== generation) return;
      return Promise.allSettled([
        ipc.subscribeTraffic(),
        ipc.subscribeConnections(),
      ]);
    });

    return () => {
      active = false;
      mounted.current = false;
      for (const un of [un1, un2, un3, un4, un5, untun])
        void un.then((f) => f());
      // React StrictMode immediately mounts this page again in development.
      // Wait for in-flight subscribe commands, then let the new generation
      // decide whether the backend streams should remain active.
      void subscribe
        .catch(() => {})
        .finally(() => {
          queueMicrotask(() => {
            if (subscriptionGeneration.current === generation) {
              ipc.unsubscribeTraffic().catch(() => {});
              ipc.unsubscribeConnections().catch(() => {});
            }
          });
        });
    };
  }, [refresh]);

  useEffect(()=>{const changed=()=>void refresh();window.addEventListener("cb:profile-switched",changed);return()=>window.removeEventListener("cb:profile-switched",changed)},[refresh]);

  // 状态自愈：事件丢失或 WS 通道冻结（内核重启竞态，#111）最迟 30s 对齐
  // 一次。重订借后端替换语义——新读线程以当前 port/secret 顶替旧通道，
  // 冻结在死端口上的旧读线程随之退场。
  useEffect(() => {
    const timer = window.setInterval(() => {
      if (!mounted.current) return;
      void refresh();
      ipc.subscribeTraffic().catch(() => {});
      ipc.subscribeConnections().catch(() => {});
    }, 30_000);
    return () => window.clearInterval(timer);
  }, [refresh]);

  const toggleProxy = async () => {
    if (busy) return;
    setBusy(true);
    try {
      await ipc.sysproxyToggle();
      await refresh();
    } catch {
      // The command reports failure through the backend-owned notification.
    } finally {
      setBusy(false);
    }
  };

  const toggleTun = async () => {
    if (busy) return;
    setBusy(true);
    try {
      await ipc.setTun(!tun.enabled);
    } catch {
      // setTun publishes its result in the backend.
    } finally {
      await refresh();
      setBusy(false);
    }
  };

  const toggleCore = async () => {
    if (busy) return;
    setBusy(true);
    try {
      if (running) {
        try {
          await ipc.coreStop();
        } catch {
          // CoreManager reports stop failures through the backend callback.
        }
      } else {
        await notify.run("explicit-core", t("home.core"),
          () => ipc.coreStart(),
          undefined, { feedback: "error-only", titleKey: "home.core" });
      }
    } finally {
      await refresh();
      setBusy(false);
    }
  };

  const canControl = ["profiles", "active", "core", "proxy", "tun"].every(key => known.has(key) && !readErrors.has(key));
  const active = profiles.find((p) => p.id === activeId);
  const latest = rates[rates.length - 1];
  const running = core.state === "Running" || core.state === "Starting";
  const routingEnabled = proxy.enabled || tun.enabled;
  const explicitOnly = explicitProxy && !tun.enabled;
  const proxyLabel = t(explicitProxy ? "linux.explicitProxy" : "studio.proxy");
  const proxyHint = explicitProxy
    ? t("linux.explicitHint", { port: mixedPort ?? "—" })
    : t(capabilities?.system_proxy ? "studio.proxyHint" : "linux.takeoverUnavailable");
  const proxyOn = explicitProxy ? core.state === "Running" : proxy.enabled;
  // 标题与副标题同源（core.state）：tun.coreRunning 独立取数，曾在重启
  // 竞态下与副标题各说各话（#111）
  const statusTitle = !canControl
    ? t(readErrors.size ? "home.stateReadFailed" : "home.statePending")
    : core.state === "Starting"
    ? t("home.coreStarting")
    : !running && routingEnabled
    ? t("home.takeoverCoreStopped")
    : explicitOnly
    ? t(core.state === "Running" ? "linux.explicitOn" : "linux.explicitOff")
    : tun.enabled
    ? running
      ? proxy.enabled
        ? t("home.tunAndSysproxyOn")
        : t("home.tunOn")
      : t("home.tunCoreStopped")
    : proxy.enabled
      ? t("home.sysproxyOn")
      : t("home.sysproxyOff");

  const statusHint = readErrors.size ? t("home.stateReadHint") : !canControl ? t("home.statePendingHint") : !active
    ? t("studio.noSource")
    : explicitOnly
      ? t("linux.explicitHint", { port: mixedPort ?? "—" })
    : !running && routingEnabled
      ? t("studio.coreUnavailable")
      : tun.enabled && proxy.enabled
        ? t("studio.bothHint")
        : tun.enabled
          ? t("studio.tunHint")
          : proxy.enabled
            ? t("studio.proxyHint")
            : t("studio.enableHint");

  const go = (page: string) =>
    window.dispatchEvent(new CustomEvent("cb:goto", { detail: page }));
  return (
    <div className="cb-home-page">
      <PageHead title={t("studio.workbench")} />
      <div className="cb-home-layout">
        <section className="cb-home-main">
          <div className="cb-home-lead">
            <div className="cb-home-status">
              <div className="cb-status-line">
                <i
                  className={`cb-dot ${core.state === "Crashed" || (routingEnabled && !running) ? "bad" : running ? "ok" : "idle"}`}
                />
                {known.has("core") ? coreText(t, core) : t("home.statePending")}
              </div>
              <span className="cb-home-mode">{known.has("mode") ? t(`mode.${mode}`) : "—"}</span>
            </div>
            <h2>{statusTitle}</h2>
            {(readErrors.size>0||!canControl||!active||!running||!routingEnabled||explicitOnly)&&<p role={readErrors.size ? "status" : undefined}>{statusHint}</p>}
            {readErrors.size > 0 && <Button size="small" onClick={() => void refresh()}>{t("home.retryState")}</Button>}
          </div>
          <section className="cb-home-path" aria-label={t("studio.path")}>
            <div className="cb-path-heading">
              <h3>{t("studio.path")}</h3>
              <span title={outbound?.chain.map(nodeNameText).join(" → ")}>
                <NodeName name={outbound?.chain[0] ?? t(`mode.${mode}`)} />
              </span>
            </div>
            <ConnectionFlow
              enabled={
                core.state === "Running" && (routingEnabled || explicitOnly) && !!outbound?.node
              }
              lastTrafficAt={lastTrafficAt}
            >
              <div>
                <span className="cb-route-mark">
                  <Icon name="device" size={32} />
                </span>
                <b>{t("studio.thisDevice")}</b>
                <small>
                  {navigator.userAgent.includes("Windows")
                    ? "Windows"
                    : navigator.userAgent.includes("Macintosh") ? "macOS" : "Linux"}
                </small>
              </div>
              <span
                className="cb-route-line"
                data-route-segment
                aria-hidden="true"
              />
              <div>
                <span className="cb-route-mark">
                  <Icon name="sliders" size={32} />
                </span>
                <b>
                  {t(
                    mode === "direct"
                      ? "studio.directRouting"
                      : mode === "global"
                        ? "studio.globalRouting"
                        : "studio.routing",
                  )}
                </b>
                <small>{t(`mode.${mode}`)}</small>
              </div>
              <span
                className="cb-route-line"
                data-route-segment
                aria-hidden="true"
              />
              <div>
                <span className="cb-route-mark">
                  <Icon name="globe" size={32} />
                </span>
                <b title={outbound?.node ? nodeNameText(outbound.node) : undefined}><NodeName name={outbound?.node ?? "—"} /></b>
                <small>
                  {outbound?.delay_ms != null
                    ? outbound.delay_ms > 0
                      ? `${outbound.delay_ms} ms`
                      : t("sites.error.timeout")
                    : coreText(t, core)}
                </small>
              </div>
            </ConnectionFlow>
          </section>
          <div className="cb-home-bottom">
            <div className="cb-home-actions">
              <Button variant="primary"  onClick={() => go("proxies")}>
                <Icon name="bolt" />
                {t("studio.chooseNode")}
              </Button>
              <Button onClick={() => go("connections")}>
                <Icon name="cube" />
                {t("studio.viewConnections")}
              </Button>
            </div>
            <section className="cb-chart-only" aria-label={t("studio.trend")}>
              <div className="cb-chart-head">
                <div>
                  <h3>{t("studio.trend")}</h3>
                  <span>{t("studio.lastMinute")} · KB/s</span>
                </div>
                <div className="cb-chart-legend">
                  <span>
                    <i className="down" />
                    {t("studio.download")}
                  </span>
                  <span>
                    <i className="up" />
                    {t("studio.upload")}
                  </span>
                </div>
              </div>
              <Sparkline rates={rates} />
              <div className="cb-chart-foot">
                <span>{t("studio.secondsAgo", { seconds: 120 })}</span>
                <span>{t("studio.secondsAgo", { seconds: 60 })}</span>
                <span>{t("studio.now")}</span>
              </div>
            </section>
          </div>
        </section>
        <div className="cb-home-side-slot">
          <aside className="cb-home-side">
            <section className="cb-takeover-panel">
              <div className="cb-home-card-title">
                <h3>{t("studio.takeover")}</h3>
                <HelpPopover label={t("studio.takeoverHelp")}><p>{t("studio.proxyHint")}</p><p>{t("studio.tunHint")}</p><p>{t("studio.independent")}</p></HelpPopover>
              </div>
              <div className="cb-takeover-row" title={proxyHint}>
                <div>
                  <b>{proxyLabel}</b>
                  <p>{proxyHint}</p>
                </div>
                <Switch checked={proxyOn} aria-label={proxyLabel}
                  onChange={() => {void (explicitProxy ? toggleCore() : toggleProxy());}}
                  disabled={!canControl || busy || core.state === "Starting" || !active || (explicitProxy ? tun.enabled : !capabilities?.system_proxy)}
                />
              </div>
              <div className="cb-takeover-row" title={t(capabilities?.tun ? (explicitProxy ? "linux.tunHint" : "studio.tunHint") : "linux.takeoverUnavailable")}>
                <div>
                  <b>{t("settings.tun")}</b>
                  <p>{t(capabilities?.tun ? (explicitProxy ? "linux.tunHint" : "studio.tunHint") : "linux.takeoverUnavailable")}</p>
                </div>
                <Switch checked={tun.enabled} aria-label={t("settings.tun")}
                  onChange={() => void toggleTun()}
                  disabled={!canControl || busy || core.state === "Starting" || !active || !capabilities?.tun}
                />
              </div>
              <div className="cb-mode-control">
                <p>{t("studio.mode")}</p>
                <SegmentedControl label={t("studio.mode")} value={known.has("mode")?mode:""} disabled={busy||!known.has("mode")||readErrors.has("mode")}
                  options={CORE_MODES.map(m=>({value:m.id,label:t(`mode.${m.id}`)}))}
                  onChange={value=>{if(!value)return;void notify.run("core-mode",notificationText("core.mode"),()=>ipc.setCoreMode(value as CoreMode),undefined,{feedback:"error-only"})}}/>

              </div>
            </section>
            <section className="cb-connection-state" aria-label={t("home.healthSummary")}>
              <div className="cb-home-card-title"><h3>{t("home.healthSummary")}</h3></div>
            <dl className="cb-health-summary" aria-label={t("home.healthSummary")}>
              <div><dt>{t("home.healthCore")}</dt><dd>{known.has("core") && !readErrors.has("core") ? coreText(t, core) : t("home.statePending")}</dd></div>
              <div><dt>{t("home.healthTakeover")}</dt><dd>{!canControl ? t("home.statePending") : routingEnabled ? t("home.takeoverConfigured") : t(explicitProxy ? "home.manualTakeover" : "home.takeoverOff")}</dd></div>
              <div title={t("home.networkScope")}><dt>{t("home.healthNetwork")}</dt><dd className="cb-health-unverified">{t("home.networkDiagnosis")}</dd></div>
            </dl>
            </section>
            <section className="cb-rate-card" aria-label={t("home.rate")}>
              <div className="cb-home-card-title">
                <h3>{t("home.rate")}</h3>
                <span className="cb-rate-state">
                  <i
                    className={`cb-dot ${core.state === "Running" ? "ok" : "idle"}`}
                  />
                  {known.has("core") ? coreText(t, core) : t("home.statePending")}
                </span>
              </div>
              <div className="cb-rate-pair">
                <div>
                  <span>↓ {t("studio.download")}</span>
                  <RateValue bytes={latest?.down ?? 0} />
                </div>
                <div>
                  <span>↑ {t("studio.upload")}</span>
                  <RateValue bytes={latest?.up ?? 0} />
                </div>
              </div>
              <div className="cb-session-strip" title={t("home.coreTotalsHint")}>
                <div>
                  <span>{t("home.coreTotalDown")}</span>
                  <b>{totals.down === null ? "—" : fmtBytes(totals.down)}</b>
                </div>
                <div>
                  <span>{t("home.coreTotalUp")}</span>
                  <b>{totals.up === null ? "—" : fmtBytes(totals.up)}</b>
                </div>
                <div>
                  <span>{t("home.connections")}</span>
                  <b>{connCount?.toLocaleString() ?? "—"}</b>
                </div>
              </div>
            </section>
          </aside>
        </div>
      </div>
    </div>
  );
}

export function PageHead({
  title,
  meta,
  children,
}: {
  title: string;
  meta?: string;
  children?: React.ReactNode;
}) {
  return (
    <header className="cb-page-head">
      <div className="cb-page-heading">
        <h1>{title}</h1>
        {meta && <p>{meta}</p>}
      </div>
      {children && <div className="cb-page-actions">{children}</div>}
    </header>
  );
}

function RateValue({ bytes }: { bytes: number }) {
  const [value, unit] = fmtBytes(bytes).split(" ");
  return (
    <p>
      <b>{value}</b>
      <small>{unit}/s</small>
    </p>
  );
}

/** 120 秒同一时间轴；短历史留空，不拉伸已有样本。 */
function Sparkline({ rates }: { rates: { up: number; down: number }[] }) {
  const W = 640;
  const H = 120;
  const peak = Math.max(1024, ...rates.map((r) => Math.max(r.up, r.down)));
  const stepKB = 10 ** Math.floor(Math.log10(peak / 1024));
  const max = Math.ceil(peak / 1024 / stepKB) * stepKB * 1024;
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
    <div className="cb-chart-drawing">
      <div className="cb-chart-axis" aria-hidden="true">
        {[1, 2 / 3, 1 / 3, 0].map((ratio) => (
          <span key={ratio}>
            {new Intl.NumberFormat("en", {
              notation: "compact",
              maximumSignificantDigits: 2,
            }).format((max * ratio) / 1024)}
          </span>
        ))}
      </div>
      <svg
        aria-hidden="true"
        viewBox={`0 0 ${W} ${H}`}
        preserveAspectRatio="none"
        className="cb-traffic-plot"
      >
        <g style={{ stroke: "var(--cb-line)" }}>
          <line x1="0" y1="6" x2={W} y2="6" />
          <line x1="0" y1="42.7" x2={W} y2="42.7" />
          <line x1="0" y1="79.3" x2={W} y2="79.3" />
          <line x1="0" y1="116" x2={W} y2="116" />
        </g>
        {upLine && (
          <>
            <path
              d={upLine}
              fill="none"
              style={{ stroke: "var(--cb-up)" }}
              strokeWidth="1.6"
            />
          </>
        )}
        {smooth("down") && (
          <path
            d={smooth("down")}
            fill="none"
            style={{ stroke: "var(--cb-down)" }}
            strokeWidth="1.4"
            opacity=".9"
          />
        )}
      </svg>
    </div>
  );
}

export function coreText(t: ReturnType<typeof useT>, s: CoreStatus): string {
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
