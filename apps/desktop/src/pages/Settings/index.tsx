import { Input, Switch, ControlRow as Row } from "@/components/Controls";
import { createReadGate, parsePort } from "@/lib/stateReliability";
import { Button } from "@/components/Button";
import { notify } from "@/notifications/service";
import { useEffect, useRef, useState } from "react";
import {
  ipc,
  type CoreBinaryInfo,
  type EngineConfigView,
  type UiSettings,
  type UpdateState,
  type PlatformCapabilities,
} from "@/services/ipc";
import Select from "@/components/Select";
import type { NavigationTarget } from "@/lib/workflow";
import { useT } from "@/i18n/i18n";
import { PageHead } from "@/pages/Home";


/** 设置页（M0-S4）：内核端口/局域网、外观主题、开机自启、内核版本。 */
export default function SettingsPage({
  section = "network",
  capabilities,
  focusTarget,
}: {
  section?: "network" | "appearance" | "logs" | "app";
  capabilities: PlatformCapabilities | null;
  focusTarget: NavigationTarget | null;
}) {
  const t = useT();
  const [engine, setEngine] = useState<EngineConfigView | null>(null);
  const [portDraft, setPortDraft] = useState<string>("");
  const [editingPort, setEditingPort] = useState(false);
  const portEditButton = useRef<HTMLButtonElement>(null);
  const [portBusy, setPortBusy] = useState(false);
  const portInFlight = useRef(false);
  const [portInvalid, setPortInvalid] = useState(false);
  const [theme, setTheme] = useState<UiSettings["theme"]>("system");
  const [autostart, setAutostart] = useState(false);
  const [appVersion, setAppVersion] = useState<string>("…");
  const [version, setVersion] = useState<string>("…");
  const [lang, setLangState] = useState<"zh" | "en">("zh");
  const [engineBin, setEngineBin] = useState<CoreBinaryInfo | null>(null);
  // 下拉框的本地选择（与持久化 engine 解耦：未安装的引擎也可选中）；
  // touched 前重挂载一律回显持久化引擎，避免切页后假显示另一引擎
  const [selectedEngine, setSelectedEngine] = useState<"mihomo" | "singbox">(
    "mihomo",
  );
  const selectedEngineRef = useRef<"mihomo" | "singbox">("mihomo");
  const engineTouched = useRef(false);
  selectedEngineRef.current = selectedEngine;
  const [flags, setFlags] = useState(true);
  const [tun, setTun] = useState<{ enabled: boolean; ready: boolean } | null>(
    null,
  );
  const [tunBusy, setTunBusy] = useState(false);
  const isMacos = navigator.userAgent.includes("Macintosh");
  const isLinux = navigator.userAgent.includes("Linux");
  const [logsSave, setLogsSave] = useState(true);
  const [logsRetention, setLogsRetention] = useState(7);
  const [autoCheck, setAutoCheck] = useState(true);
  const [autoDownload, setAutoDownload] = useState(false);
  const [updateChecked, setUpdateChecked] = useState(false);
  const [updateState, setUpdateState] = useState<UpdateState | null>(null);
  const [updateChecking, setUpdateChecking] = useState(false);
  const updateCheckInFlight = useRef(false);
  const [lightweight, setLightweight] = useState(false);
  const [installing, setInstalling] = useState<string | null>(null);

  const [settingsKnown, setSettingsKnown] = useState<Set<string>>(new Set());
  const [settingsErrors, setSettingsErrors] = useState<Set<string>>(new Set());
  const requiredSettings = { network: ["engine", "tun", "binary"], appearance: ["ui"], logs: ["logs"], app: ["ui", "autostart", "updates"] }[section];
  const settingsReady = requiredSettings.every(key => settingsKnown.has(key) && !settingsErrors.has(key));
  const settingsReadFailed = requiredSettings.some(key => settingsErrors.has(key));
  useEffect(()=>{
    const focusControl = focusTarget?.control;
    if(!focusControl||!settingsReady)return;
    const frame = requestAnimationFrame(() => {
      const row = document.getElementById(focusControl);
      row?.scrollIntoView({ block: "center" });
      const control = row?.querySelector<HTMLElement>(
        'button:not(:disabled), input:not(:disabled), select:not(:disabled), a[href]',
      );
      control?.focus({ preventScroll: true });
    });
    return()=>cancelAnimationFrame(frame);
  },[focusTarget,settingsReady,section]);
  const [savingCount, setSavingCount] = useState(0);
  const settingsGate = useRef(createReadGate());
  const dirtyPort = useRef(false);
  const latestTunEvent = useRef<boolean | null>(null);
  const load = async () => {
    const revision = settingsGate.current.begin("load");
    latestTunEvent.current = null;
    let failed = false;
    const read = async <T,>(key: string, task: () => Promise<T>, commit: (value: T) => void) => {
      const fieldRevision = settingsGate.current.begin(key);
      const current = () => settingsGate.current.current("load", revision) && settingsGate.current.current(key, fieldRevision);
      try {
        const value = await task();
        if (!current()) return;
        commit(value);
        setSettingsKnown(old => new Set([...old, key]));
        setSettingsErrors(old => { const next = new Set(old); next.delete(key); return next; });
      } catch {
        if (!current()) return;
        failed = true;
        setSettingsErrors(old => new Set([...old, key]));
      }
    };
    await Promise.all([
      read("engine", ipc.getEngineConfig, e => {
        setEngine(e); setFlags(e.flag_emoji);
        if (!dirtyPort.current) setPortDraft(String(e.mixed_port));
        if (!engineTouched.current) { setSelectedEngine(e.engine); selectedEngineRef.current = e.engine; }
      }),
      read("ui", ipc.getUiSettings, ui => { setLangState(ui.lang as "zh" | "en"); setLightweight(ui.lightweight_close); setTheme(ui.theme); }),
      read("autostart", ipc.autostartStatus, setAutostart),
      read("tun", ipc.tunStatus, value => setTun({ ...value, enabled: latestTunEvent.current ?? value.enabled })),
      read("logs", ipc.logsSettings, lg => { setLogsSave(lg.save_enabled); setLogsRetention(lg.retention_days); }),
      read("updates", ipc.updateSettingsGet, u => { setAutoCheck(u.auto_check); setAutoDownload(u.auto_download); }),
    ]);
    await read("binary", () => ipc.coreBinaryInfoFor(selectedEngineRef.current), info => { setEngineBin(info); setVersion(info.version || t("settings.notInstalled")); });
    if (!settingsGate.current.current("load", revision)) return;
    if (failed) notify.error(t("settings.readFailed"), "settings-read");
    else void notify.resolve("settings-read");
    void ipc.updateStateGet().then(setUpdateState).catch(e => notify.error(e, "update-state-read"));
  };

  useEffect(() => {
    load();
    let alive = true;
    ipc
      .appVersion()
      .then((version) => {
        if (alive) setAppVersion(version);
      })
      .catch((error) => {
        if (alive) {
          setAppVersion("—");
          notify.error(error, "app-version-read");
        }
      });
    return () => {
      alive = false;
      settingsGate.current.invalidate("load");
    };
  }, []);

  useEffect(() => {
    let alive = true;
    const un = ipc.onTunStatus((enabled) => {
      if (!alive) return;
      latestTunEvent.current = enabled;
      setTun(current => ({ enabled, ready: current?.ready ?? false }));
    });
    return () => {
      alive = false;
      void un.then((off) => off());
    };
  }, []);

  useEffect(() => {
    const un = ipc.onUpdateState((state) => {
      setUpdateState(state);
    });
    return () => {
      void un.then((off) => off());
    };
  }, []);

  const saving = useRef(new Set<string>());
  const saveSetting = async (
    key: string,
    title: string,
    task: () => Promise<unknown>,
    commit: () => void,
    success?: string,
  ) => {
    if (saving.current.has(key)) return;
    saving.current.add(key);
    setSavingCount(count => count + 1);
    try {
      const result = await notify.run(`setting-${key}`, title, task, success, {
        feedback: "error-only",
        titleKey:
          (
            {
              logs: "settings.logsSave",
              retention: "settings.logsRetention",
              "auto-check": "settings.autoCheck",
              "auto-download": "settings.autoDownload",
            } as Record<string, string>
          )[key] ?? `settings.${key}`,
      });
      if (result.ok) commit();
      else await load();
    } finally {
      saving.current.delete(key);
      setSavingCount(count => Math.max(0, count - 1));
    }
  };

  const runUpdateCheck = async () => {
    if (updateCheckInFlight.current) return;
    updateCheckInFlight.current = true;
    setUpdateChecking(true);

    try {
      const version = await ipc.updateCheckNow();
      const state = await ipc.updateStateGet();
      setUpdateState(
        state ??
          (version
            ? { version, notes: null, downloaded: false, progress: null }
            : null),
      );
      setUpdateChecked(true);
      void notify.resolve("update-check");
    } catch (error) {
      notify.error(
        String(error) === "UPDATE_CHECK_TIMEOUT"
          ? t("settings.updateTimeout")
          : error,
        "update-check",
        t("settings.updateFailed"),
      );
    } finally {
      updateCheckInFlight.current = false;
      setUpdateChecking(false);
    }
  };

  const closePortEditor = () => {
    if (portInFlight.current) return;
    dirtyPort.current = false;
    setPortDraft(engine ? String(engine.mixed_port) : "");
    setPortInvalid(false);
    setEditingPort(false);
    requestAnimationFrame(() => portEditButton.current?.focus());
  };

  const applyPort = async () => {
    if (portInFlight.current || !engine) return;
    const port = parsePort(portDraft);
    if (port === null) {
      setPortInvalid(true);
      notify.error(
        t("settings.invalidPort"),
        "setting-port",
        t("settings.port"),
      );
      return;
    }
    if (port === engine?.mixed_port) { closePortEditor(); return; }
    portInFlight.current = true; setPortBusy(true); setPortInvalid(false);
    try { await saveSetting(
      "port",
      t("settings.port"),
      () => ipc.setMixedPort(port),
      () => {
        setEngine((old) => (old ? { ...old, mixed_port: port } : old));
        dirtyPort.current = false;
        setEditingPort(false);
        requestAnimationFrame(() => portEditButton.current?.focus());
      },
      `${t("settings.port")}: ${port}`,
    ); } finally { portInFlight.current = false; setPortBusy(false); }
  };

  return (
    <div className={`cb-settings-page cb-settings-${section}`}>
      <PageHead title={t("settings.title")} meta={!settingsReady ? t(settingsReadFailed ? "settings.readFailed" : "home.statePendingHint") : undefined}>
        {settingsReadFailed && <Button onClick={() => void load()}>{t("home.retryState")}</Button>}
      </PageHead>
      <fieldset className="cb-settings-fields" disabled={!settingsReady || savingCount > 0} aria-busy={savingCount > 0 || (!settingsReady && !settingsReadFailed)}>

      {section === "network" && (
        <><Group title={t("settings.coreSection")}>
          <Row label={t("settings.engine")} hint={t("settings.engineHint")}>
            <Select
              label={t("settings.engine")}
              value={selectedEngine}
              onChange={(v) => {
                // 仅更新本地选择：安装状态随所选引擎显示；
                // 已安装的引擎在下载行位置出现「启用」按钮
                const val = v as "mihomo" | "singbox";
                engineTouched.current = true;
                setSelectedEngine(val);
                void ipc
                  .coreBinaryInfoFor(val)
                  .then((info) => {
                    setEngineBin(info);
                    setVersion(info?.version || t("settings.notInstalled"));
                  })
                  .catch((e) => notify.error(e, "engine-info"));
              }}
              options={[
                { value: "mihomo", label: "mihomo (Clash Meta)" },
                { value: "singbox", label: "sing-box" },
              ]}
            />
          </Row>
          {engineBin?.source === "missing" ? (
            <Row
              label={t("settings.coreMissingEngine", {
                engine: selectedEngine === "singbox" ? "sing-box" : "mihomo",
              })}
            >
              <Button variant="primary" loading={installing !== null}
                disabled={installing !== null}
                onClick={async () => {
                  if (installing !== null) return;
                  setInstalling("starting…");
                  try {
                    const info = await ipc.coreInstall(selectedEngine);
                    setEngineBin(info);
                    setVersion(info.version || t("settings.coreReady"));
                    // Activation has its own lifecycle after the download completes.
                    await saveSetting(
                      "engine",
                      t("settings.engine"),
                      () => ipc.setEngine(selectedEngine),
                      () => {
                        void load();
                      },
                    );
                  } catch (e) {
                    // 失败：回下载行（engineBin 重新查询所选引擎 → 仍是 missing）
                    notify.error(
                      e,
                      "core-download",
                      t("settings.downloadCore"),
                    );
                  } finally {
                    setInstalling(null);
                    setEngineBin(
                      await ipc
                        .coreBinaryInfoFor(selectedEngineRef.current)
                        .catch(() => null),
                    );
                    await load();
                  }
                }}
              >
                {installing ?? t("settings.downloadCore")}
              </Button>
            </Row>
          ) : (
            selectedEngine !== (engine?.engine ?? "mihomo") && (
              <Row
                label={t("settings.engineApply", { engine: selectedEngine === "singbox" ? "sing-box" : "mihomo" })} hint={t("layout.engineImpact")}
              >
                <Button variant="primary"
                  onClick={async () => {
                    await saveSetting(
                      "engine",
                      t("settings.engine"),
                      () => ipc.setEngine(selectedEngine),
                      () => {
                        void load();
                      },
                    );
                  }}
                >
                  {t("settings.activateEngine")}
                </Button>
              </Row>
            )
          )}
          <Row label={t("settings.version")} hint={selectedEngine === "singbox" ? "sing-box" : "mihomo"}>
            <span
              className="cb-selectable text-xs"
              style={{ color: "var(--cb-text-dim)" }}
            >
              {engineBin?.source === "missing"
                ? t("settings.notInstalled")
                : version}
            </span>
          </Row>
        </Group><Group title={t("layout.connectionSettings")}>
          <Row id="cb-port-setting" label={t("settings.port")} hint={t("settings.portHint")}>
            <div className="cb-port-editor">
              {editingPort ? <div className="cb-port-editor-actions">
                <Input id="mixed-port" autoFocus aria-label={t("settings.port")} inputMode="numeric"
                  value={portDraft} onChange={e => { dirtyPort.current = true; setPortDraft(e.target.value); setPortInvalid(false); }}
                  aria-invalid={portInvalid} aria-describedby="mixed-port-help"
                  disabled={portBusy || !engine}
                  onKeyDown={e => { if (e.key === "Enter") { e.preventDefault(); void applyPort(); } else if(e.key === "Escape") { e.preventDefault(); closePortEditor(); } }} />
                <Button size="small" loading={portBusy} disabled={portBusy || !engine || portDraft === String(engine.mixed_port)} onClick={() => void applyPort()}>{t("settings.applyPort")}</Button>
                <Button size="small" disabled={portBusy} onClick={closePortEditor}>{t("settings.cancelPort")}</Button>
              </div> : <div className="cb-port-readout">
                <span className="cb-port-value">{engine?.mixed_port ?? "—"}</span>
                <Button ref={portEditButton} size="small" disabled={!engine} aria-label={`${t("settings.editPort")} ${t("settings.port")}`} onClick={() => setEditingPort(true)}>{t("settings.editPort")}</Button>
              </div>}
              {editingPort && <p id="mixed-port-help" className="cb-control-note" role="status">{t(portInvalid ? "settings.invalidPort" : portBusy ? "settings.portApplying" : "settings.portApplyHint")}</p>}
            </div>
          </Row>
          <Row label={t("settings.flags")} hint={t("settings.flagsHint")}>
            <Switch
              checked={flags}
              onChange={async (v) => {
                await saveSetting(
                  "flags",
                  t("settings.flags"),
                  () => ipc.setFlagEmoji(v),
                  () => setFlags(v),
                );
              }}
            />
          </Row>
          <Row
            label={t("settings.tun")}
            hint={!capabilities?.tun ? t("linux.takeoverUnavailable") : tunBusy ? t("settings.tunSwitching") : t(isLinux ? "linux.tunHint" : "settings.tunHint")}
          >
            <div className="flex items-center gap-2">
              {tunBusy && (
                <svg
                  className="h-3 w-3 animate-spin"
                  viewBox="0 0 24 24"
                  fill="none"
                  stroke="currentColor"
                  strokeWidth="2"
                  aria-hidden="true"
                >
                  <path d="M21 12a9 9 0 1 1-9-9" strokeLinecap="round" />
                </svg>
              )}
              {tunBusy && (
                <span className="text-xs text-[var(--cb-text-dim)]">
                  {t("settings.tunSwitching")}
                </span>
              )}
              <Switch
                id="cb-tun-control"
                checked={tun?.enabled ?? false}
                disabled={tunBusy || !capabilities?.tun}
                onChange={async (v) => {
                  if (tunBusy) return;
                  setTunBusy(true);
                  try {
                    await ipc.setTun(v);
                    setTun(await ipc.tunStatus());
                  } catch {
                    // setTun publishes its result in the backend.
                  } finally {
                    await load();
                    setTunBusy(false);
                  }
                }}
              />
            </div>
          </Row>
          {((isMacos && tun?.enabled) || isLinux) && (
            <Row
              label={t("settings.tunState")}
              hint={t(isLinux ? "linux.tunHint" : "settings.tunStateHint")}
            >
              <span
                className="text-xs"
                style={{ color: tun?.ready ? "var(--cb-ok)" : "var(--cb-warn)" }}
              >
                {isLinux ? t(tun?.ready ? "linux.serviceReady" : "linux.serviceMissing")
                  : tun?.ready ? t("settings.tunReady") : t("settings.tunNeedAuth")}
              </span>
            </Row>
          )}
          {isLinux && capabilities?.tun && (
            <Row
              label={t("linux.wireguardCoexist")}
              hint={t(tun?.enabled ? "linux.wireguardCoexistTurnOffTun" : "linux.wireguardCoexistHint")}
            >
              <Switch
                checked={engine?.linux_wireguard_coexist ?? false}
                disabled={tunBusy || !tun || tun.enabled}
                onChange={async (enabled) => {
                  const key = "linux-wireguard-coexist";
                  if (saving.current.has(key)) return;
                  saving.current.add(key);
                  setSavingCount(count => count + 1);
                  try {
                    await ipc.setLinuxWireguardCoexist(enabled);
                  } catch {
                    // The backend reports the error. Read the stored value below.
                  } finally {
                    try {
                      await load();
                    } finally {
                      saving.current.delete(key);
                      setSavingCount(count => Math.max(0, count - 1));
                    }
                  }
                }}
              />
            </Row>
          )}
          <Row label={t("settings.lan")} hint={t("settings.lanHint")}>
            <Switch
              checked={engine?.allow_lan ?? false}
              onChange={async (v) => {
                await saveSetting(
                  "lan",
                  t("settings.lan"),
                  () => ipc.setAllowLan(v),
                  () => {
                    void load();
                  },
                  v ? t("settings.lanOn") : t("settings.lanOff"),
                );
              }}
            />
          </Row>
        </Group></>
      )}

      {section === "appearance" && (
        <Group title={t("settings.appearance")}>
          <Row label={t("settings.theme")}>
            <Select
              label={t("settings.theme")}
              value={theme}
              options={[
                { value: "system", label: t("theme.system") },
                { value: "light", label: t("theme.light") },
                { value: "dark", label: t("theme.dark") },
              ]}
              onChange={async (v) => {
                const next = v as UiSettings["theme"];
                await saveSetting(
                  "theme",
                  t("settings.theme"),
                  () => ipc.setTheme(next),
                  () => setTheme(next),
                );
              }}
            />
          </Row>
          <Row label={t("settings.lang")}>
            <Select
              label={t("settings.lang")}
              value={lang}
              options={[
                { value: "zh", label: "简体中文" },
                { value: "en", label: "English" },
              ]}
              onChange={async (v) => {
                const l = v as "zh" | "en";
                await saveSetting(
                  "lang",
                  t("settings.lang"),
                  () => ipc.setLang(l),
                  () => setLangState(l),
                );
              }}
            />
          </Row>
        </Group>
      )}

      {section === "logs" && (
        <Group title={t("settings.logsSection")}>
          <Row label={t("settings.logsSave")} hint={t("settings.logsSaveHint")}>
            <Switch
              checked={logsSave}
              onChange={async (v) => {
                await saveSetting(
                  "logs",
                  t("settings.logsSave"),
                  () => ipc.setLogsSave(v),
                  () => setLogsSave(v),
                );
              }}
            />
          </Row>
          <Row
            label={t("settings.logsRetention")}
            hint={t("settings.logsRetentionHint")}
          >
            <Select
              label={t("settings.logsRetention")}
              value={String(logsRetention)}
              options={[1, 3, 5, 7, 15, 30].map((d) => ({
                value: String(d),
                label: `${d} ${t("settings.logsDays")}`,
              }))}
              onChange={async (v) => {
                const d = Number(v);
                await saveSetting(
                  "retention",
                  t("settings.logsRetention"),
                  () => ipc.setLogsRetention(d),
                  () => setLogsRetention(d),
                );
              }}
            />
          </Row>
          <Row label={t("settings.logsOpen")} hint={t("settings.logsOpenHint")}>
            <Button
              onClick={() =>
                ipc.openLogsDir().catch((e) => notify.error(e, "open-logs"))
              }
            >
              {t("settings.logsOpenBtn")}
            </Button>
          </Row>
        </Group>
      )}

      {section === "app" && (
        <><Group title={t("settings.systemSection")}>
          <Row
            label={t("settings.lightweight")}
            hint={t("settings.lightweightHint")}
          >
            <Switch
              checked={lightweight}
              onChange={async (v) => {
                await saveSetting(
                  "lightweight",
                  t("settings.lightweight"),
                  () => ipc.setLightweightClose(v),
                  () => setLightweight(v),
                );
              }}
            />
          </Row>
          <Row
            label={t("settings.autostart")}
            hint={t("settings.autostartHint")}
          >
            <Switch
              checked={autostart}
              onChange={async (v) => {
                await saveSetting(
                  "autostart",
                  t("settings.autostart"),
                  () => ipc.autostartSet(v),
                  () => setAutostart(v),
                  v ? t("settings.autostartOn") : t("settings.autostartOff"),
                );
              }}
            />
          </Row>
        </Group><Group title={t("layout.maintenance")}>
          <Row
            label={t("settings.snapshot")}
            hint={t("settings.snapshotAuto")}
          />
          <Row label={t("settings.appVersion")} hint="Crossbow">
            <span
              className="text-xs tabular-nums"
              style={{ fontFamily: "var(--cb-mono)", color: "var(--cb-text)" }}
            >
              {appVersion === "…" || appVersion === "—"
                ? appVersion
                : `v${appVersion}`}
            </span>
          </Row>
          <Row
            label={t("settings.autoCheck")}
            hint={t(capabilities?.app_update ? "settings.autoCheckHint" : "linux.updateUnavailable")}
          >
            <Switch
              checked={Boolean(capabilities?.app_update && autoCheck)}
              disabled={!capabilities?.app_update}
              onChange={async (v) => {
                await saveSetting(
                  "auto-check",
                  t("settings.autoCheck"),
                  () => ipc.setUpdateAutoCheck(v),
                  () => setAutoCheck(v),
                );
              }}
            />
          </Row>
          <Row
            label={t("settings.autoDownload")}
            hint={t(capabilities?.app_update ? "settings.autoDownloadHint" : "linux.updateUnavailable")}
          >
            <Switch
              checked={Boolean(capabilities?.app_update && autoDownload)}
              disabled={!capabilities?.app_update}
              onChange={async (v) => {
                await saveSetting(
                  "auto-download",
                  t("settings.autoDownload"),
                  () => ipc.setUpdateAutoDownload(v),
                  () => setAutoDownload(v),
                );
              }}
            />
          </Row>
          <Row id="cb-update-status"
            label={t("settings.updateNow")}
            hint={
              !capabilities?.app_update ? t("linux.updateUnavailable") : updateState
                ? updateState.downloaded
                  ? t("settings.updateReady")
                  : updateState.progress
                    ? updateState.progress[1] > 0
                      ? `${t("settings.updateProgress")} ${Math.min(100, Math.floor(
                          (updateState.progress[0] / updateState.progress[1]) * 100,
                        ))}%`
                      : t("settings.updateProgress")
                    : t("settings.newVersion", { v: updateState.version })
                : updateChecked
                  ? t("settings.upToDate")
                  : undefined
            }
          >
            <div className="flex max-w-[240px] flex-col items-end gap-1.5">
              <div className="flex items-center gap-2">
                {(updateState === null || updateChecking) && (
                  <Button
                    onClick={runUpdateCheck}
                    disabled={updateChecking || !capabilities?.app_update}
                    loading={updateChecking}
                  >
                    {t(
                      updateChecking
                        ? "settings.updateChecking"
                        : "settings.checkUpdateBtn",
                    )}
                  </Button>
                )}
                {updateState && !updateState.downloaded && !updateChecking && (
                  <Button
                    disabled={updateState.progress !== null}
                    onClick={() =>
                      ipc
                        .updateStartDownload()
                        .catch((e) => notify.error(e, "update-download"))
                    }
                  >
                    {t("settings.updateDownload")}
                  </Button>
                )}
                {updateState?.downloaded && !updateChecking && (
                  <Button

                    onClick={() =>
                      notify.run(
                        "update-install",
                        t("settings.updateRestart"),
                        () => ipc.updateInstall(),
                      )
                    }
                  >
                    {t("settings.updateRestart")}
                  </Button>
                )}
              </div>
            </div>
          </Row>
        </Group></>
      )}
      </fieldset>
    </div>
  );
}

function Group({
  title,
  children,
}: {
  title: string;
  children: React.ReactNode;
}) {
  return (
    // 不能加 overflow-hidden：会把组内 Select 的绝对定位弹层在卡片边界裁掉
    <section className="cb-settings-group">
      <div
        className="border-b px-4 py-2.5 cb-micro"
        style={{ borderColor: "var(--cb-line)" }}
      >
        {title}
      </div>
      <div className="cb-rows">{children}</div>
    </section>
  );
}
