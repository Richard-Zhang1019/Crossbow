import { useEffect, useRef, useState } from "react";
import { ipc, type CoreBinaryInfo, type EngineConfigView, type UiSettings } from "../ipc";
import Select from "../components/Select";
import { useT } from "../i18n";
import { PageHead } from "./Home";

/** 设置页（M0-S4）：内核端口/局域网、外观主题、开机自启、内核版本。 */
export default function SettingsPage() {
  const t = useT();
  const [engine, setEngine] = useState<EngineConfigView | null>(null);
  const [portDraft, setPortDraft] = useState<string>("");
  const [theme, setTheme] = useState<UiSettings["theme"]>("system");
  const [autostart, setAutostart] = useState(false);
  const [version, setVersion] = useState<string>("…");
  const [lang, setLangState] = useState<"zh" | "en">("zh");
  const [engineBin, setEngineBin] = useState<CoreBinaryInfo | null>(null);
  // 下拉框的本地选择（与持久化 engine 解耦：未安装的引擎也可选中）
  const [selectedEngine, setSelectedEngine] = useState<"mihomo" | "singbox">("mihomo");
  const selectedEngineRef = useRef<"mihomo" | "singbox">("mihomo");
  selectedEngineRef.current = selectedEngine;
  const [flags, setFlags] = useState(true);
  const [lightweight, setLightweight] = useState(false);
  const [installing, setInstalling] = useState<string | null>(null);
  const [msg, setMsg] = useState<{ ok: boolean; text: string; full?: string } | null>(null);

  const load = async () => {
    const e = await ipc.getEngineConfig().catch(() => null);
    setEngine(e);
    setPortDraft(e ? String(e.mixed_port) : "");
    const ui = await ipc.getUiSettings().catch(() => null);
    if (ui) {
      setLangState((ui.lang as "zh" | "en") ?? "zh");
      setLightweight(ui.lightweight_close);
    }
    setTheme(await ipc.getUiSettings().then((u) => u.theme).catch(() => "system" as const));
    setAutostart(await ipc.autostartStatus().catch(() => false));
    if (e) setFlags(e.flag_emoji);
    // engineBin 始终跟随本地选择的引擎（失败/成功状态归一）；
    // 持久化引擎由 set_engine 成功后才变
    const target = selectedEngineRef.current ?? (e?.engine as "mihomo" | "singbox") ?? "mihomo";
    setEngineBin(await ipc.coreBinaryInfoFor(target).catch(() => null));
    setVersion(await ipc.coreVersion().catch(() => t("settings.notInstalled")));
  };

  useEffect(() => {
    load();
  }, []);

  const flash = (ok: boolean, text: string, full?: string) => {
    setMsg({ ok, text, full: full ?? text });
    setTimeout(() => setMsg(null), 6000);
  };

  const applyPort = async () => {
    const port = Number(portDraft);
    if (!Number.isInteger(port) || port < 1024 || port > 65535) {
      flash(false, t("settings.invalidPort"));
      return;
    }
    try {
      await ipc.setMixedPort(port);
      flash(true, `${t("settings.port")}: ${port}`);
    } catch (e) {
      flash(false, String(e));
    }
  };

  return (
    <div className="mx-auto max-w-3xl space-y-5">
      <PageHead title={t("settings.title")}>
        {msg && (
          <span
            className="text-xs"
            style={{ color: msg.ok ? "var(--cb-ok)" : "var(--cb-bad)" }}
            title={msg.full ?? undefined}
          >
            {msg.text}
          </span>
        )}
      </PageHead>

      <Group title={t("settings.coreSection")}>
        <Row label={t("settings.engine")} hint={t("settings.engineHint")}>
          <select
            value={selectedEngine}
            onChange={(e) => {
              // 仅更新本地选择：安装状态随所选引擎显示；
              // 已安装的引擎在下载行位置出现「启用」按钮
              const v = e.target.value as "mihomo" | "singbox";
              setSelectedEngine(v);
              void ipc
                .coreBinaryInfoFor(v)
                .then(setEngineBin)
                .catch(() => {});
            }}
            className="cb-input"
          >
            <option value="mihomo">mihomo (Clash Meta)</option>
            <option value="singbox">sing-box</option>
          </select>
        </Row>
        {engineBin?.source === "missing" ? (
          <Row
            label={t("settings.coreMissingEngine", { engine: selectedEngine })}
          >
            <button
              disabled={installing !== null}
              onClick={async () => {
                setInstalling("starting…");
                const un = await ipc.onInstallProgress((p) =>
                  setInstalling(
                    p.stage === "downloading" ? t("settings.downloading") : p.stage,
                  ),
                );
                try {
                  const info = await ipc.coreInstall(selectedEngine);
                  setEngineBin(info);
                  setVersion(info.version || t("settings.coreReady"));
                  flash(true, t("settings.coreReady"));
                  // 下载完成 → 应用切换
                  await ipc.setEngine(selectedEngine);
                  await load();
                  flash(true, `engine: ${selectedEngine}`);
                } catch (e) {
                  // 失败：回下载行（engineBin 重新查询所选引擎 → 仍是 missing）
                  const raw = String(e);
                  flash(
                    false,
                    raw.length > 140 ? `${raw.slice(0, 140)}…（悬停查看完整错误）` : raw,
                    raw,
                  );
                } finally {
                  un();
                  setInstalling(null);
                  setEngineBin(
                    await ipc
                      .coreBinaryInfoFor(selectedEngineRef.current)
                      .catch(() => null),
                  );
                  await load();
                }
              }}
              className="cb-btn acc px-3 py-1.5"
            >
              {installing ?? t("settings.downloadCore")}
            </button>
          </Row>
        ) : (
          selectedEngine !== (engine?.engine ?? "mihomo") && (
            <Row label={t("settings.engineApply", { engine: selectedEngine })}>
              <button
                className="cb-btn acc px-3 py-1.5"
                onClick={async () => {
                  try {
                    await ipc.setEngine(selectedEngine);
                    await load();
                    flash(true, `engine: ${selectedEngine}`);
                  } catch (e) {
                    flash(false, String(e));
                  }
                }}
              >
                启用
              </button>
            </Row>
          )
        )}
        <Row label={t("settings.version")} hint={selectedEngine}>
          <span className="cb-selectable text-xs" style={{ color: "var(--cb-text-dim)" }}>
            {engineBin?.source === "missing"
              ? t("settings.notInstalled")
              : version}
          </span>
        </Row>
        <Row label={t("settings.port")} hint={t("settings.portHint")}>
          <input
            value={portDraft}
            onChange={(e) => setPortDraft(e.target.value)}
            onBlur={applyPort}
            onKeyDown={(e) => e.key === "Enter" && applyPort()}
            className="cb-input cb-selectable w-24 px-2 py-1"
          />
        </Row>
        <Row label={t("settings.flags")} hint={t("settings.flagsHint")}>
          <Switch
            checked={flags}
            onChange={async (v) => {
              await ipc.setFlagEmoji(v).catch((e) => flash(false, String(e)));
              setFlags(v);
              flash(true, v ? "ON" : "OFF");
            }}
          />
        </Row>
        <Row label={t("settings.lan")} hint={t("settings.lanHint")}>
          <Switch
            checked={engine?.allow_lan ?? false}
            onChange={async (v) => {
              await ipc.setAllowLan(v).catch((e) => flash(false, String(e)));
              load();
              flash(true, v ? t("settings.lanOn") : t("settings.lanOff"));
            }}
          />
        </Row>
      </Group>

      <Group title={t("settings.appearance")}>
        <Row label={t("settings.theme")}>
          <Select
            value={theme}
            options={[
              { value: "system", label: t("theme.system") },
              { value: "light", label: t("theme.light") },
              { value: "dark", label: t("theme.dark") },
            ]}
            onChange={async (v) => {
              const t = v as UiSettings["theme"];
              await ipc.setTheme(t);
              setTheme(t);
            }}
          />
        </Row>
        <Row label={t("settings.lang")}>
          <Select
            value={lang}
            options={[
              { value: "zh", label: "简体中文" },
              { value: "en", label: "English" },
            ]}
            onChange={async (v) => {
              const l = v as "zh" | "en";
              await ipc.setLang(l).catch((e) => flash(false, String(e)));
              setLangState(l);
            }}
          />
        </Row>
      </Group>

      <Group title={t("settings.systemSection")}>
        <Row label={t("settings.lightweight")} hint={t("settings.lightweightHint")}>
          <Switch
            checked={lightweight}
            onChange={async (v) => {
              await ipc.setLightweightClose(v).catch((e) => flash(false, String(e)));
              setLightweight(v);
              flash(true, v ? "ON" : "OFF");
            }}
          />
        </Row>
        <Row label={t("settings.autostart")} hint={t("settings.autostartHint")}>
          <Switch
            checked={autostart}
            onChange={async (v) => {
              const ok = await ipc.autostartSet(v).then(() => true).catch((e) => {
                flash(false, String(e));
                return false;
              });
              if (ok) {
                setAutostart(v);
                flash(true, v ? t("settings.autostartOn") : t("settings.autostartOff"));
              }
            }}
          />
        </Row>
        <Row label={t("settings.snapshot")} hint={t("settings.snapshotAuto")} />
        <Row label={t("settings.checkUpdate")}>
          <button
            onClick={async () => {
              try {
                const v = await ipc.checkUpdate();
                flash(true, v ? t("settings.newVersion", { v }) : t("settings.upToDate"));
              } catch {
                flash(false, t("settings.updateFailed"));
              }
            }}
            className="cb-btn py-1.5"
          >
            Go
          </button>
        </Row>
      </Group>
    </div>
  );
}

function Group({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    // 不能加 overflow-hidden：会把组内 Select 的绝对定位弹层在卡片边界裁掉
    <section className="cb-card">
      <div className="border-b px-4 py-2.5 cb-micro"
        style={{ borderColor: "var(--cb-line)" }}>
        {title}
      </div>
      <div className="cb-rows">
        {children}
      </div>
    </section>
  );
}

function Row({ label, hint, children }: { label: string; hint?: string; children?: React.ReactNode }) {
  return (
    <div className="flex items-center justify-between px-4 py-3">
      <div>
        <div className="text-sm">{label}</div>
        {hint && (
          <div className="mt-0.5 text-xs" style={{ color: "var(--cb-text-dim)" }}>
            {hint}
          </div>
        )}
      </div>
      {children}
    </div>
  );
}

function Switch({ checked, onChange }: { checked: boolean; onChange: (v: boolean) => void }) {
  return (
    <button
      onClick={() => onChange(!checked)}
      className={`cb-toggle ${checked ? "on" : ""}`}
    />
  );
}
