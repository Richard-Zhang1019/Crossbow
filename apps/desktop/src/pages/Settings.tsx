import { useEffect, useState } from "react";
import { ipc, type CoreBinaryInfo, type EngineConfigView, type UiSettings } from "../ipc";
import { useT } from "../i18n";

/** 设置页（M0-S4）：内核端口/局域网、外观主题、开机自启、内核版本。 */
export default function SettingsPage() {
  const t = useT();
  const [engine, setEngine] = useState<EngineConfigView | null>(null);
  const [portDraft, setPortDraft] = useState<string>("");
  const [theme, setTheme] = useState<UiSettings["theme"]>("system");
  const [autostart, setAutostart] = useState(false);
  const [version, setVersion] = useState<string>("…");
  const [lang, setLangState] = useState<"zh" | "en">("zh");
  const [coreBin, setCoreBin] = useState<CoreBinaryInfo | null>(null);
  const [installing, setInstalling] = useState<string | null>(null);
  const [msg, setMsg] = useState<{ ok: boolean; text: string } | null>(null);

  const load = async () => {
    const e = await ipc.getEngineConfig().catch(() => null);
    setEngine(e);
    setPortDraft(e ? String(e.mixed_port) : "");
    const ui = await ipc.getUiSettings().catch(() => null);
    if (ui) setLangState((ui.lang as "zh" | "en") ?? "zh");
    setTheme(await ipc.getUiSettings().then((u) => u.theme).catch(() => "system" as const));
    setAutostart(await ipc.autostartStatus().catch(() => false));
    setCoreBin(await ipc.coreBinaryInfo().catch(() => null));
    setVersion(await ipc.coreVersion().catch(() => t("settings.notInstalled")));
  };

  useEffect(() => {
    load();
  }, []);

  const flash = (ok: boolean, text: string) => {
    setMsg({ ok, text });
    setTimeout(() => setMsg(null), 2500);
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
      <div className="flex items-center justify-between">
        <h1 className="text-lg font-semibold">{t("settings.title")}</h1>
        {msg && (
          <span className="text-xs" style={{ color: msg.ok ? "#34c759" : "#e0524c" }}>
            {msg.text}
          </span>
        )}
      </div>

      <Group title={t("settings.coreSection")}>
        {coreBin?.source === "missing" && (
          <Row label={t("settings.coreMissing")}>
            <button
              disabled={installing !== null}
              onClick={async () => {
                setInstalling("starting…");
                const un = await ipc.onInstallProgress((p) =>
                  setInstalling(
                    p.stage === "downloading"
                      ? `${t("settings.downloading")} ${Math.round((p.detail.length % 100))}%`
                      : p.stage,
                  ),
                );
                try {
                  const info = await ipc.coreInstall();
                  setCoreBin(info);
                  setVersion(info.version || t("settings.coreReady"));
                  flash(true, t("settings.coreReady"));
                } catch (e) {
                  flash(false, String(e));
                } finally {
                  un();
                  setInstalling(null);
                }
              }}
              className="rounded-lg px-3 py-1.5 text-xs font-medium text-white disabled:opacity-40"
              style={{ background: "var(--cb-accent)" }}
            >
              {installing ?? t("settings.downloadCore")}
            </button>
          </Row>
        )}
        <Row label={t("settings.version")} hint="mihomo sidecar">
          <span className="cb-selectable text-xs" style={{ color: "var(--cb-text-dim)" }}>
            {coreBin?.source === "missing" ? t("settings.notInstalled") : version}
          </span>
        </Row>
        <Row label={t("settings.port")} hint={t("settings.portHint")}>
          <input
            value={portDraft}
            onChange={(e) => setPortDraft(e.target.value)}
            onBlur={applyPort}
            onKeyDown={(e) => e.key === "Enter" && applyPort()}
            className="cb-selectable w-24 rounded-lg border px-2 py-1 text-sm outline-none focus:border-[var(--cb-accent)]"
            style={{ borderColor: "var(--cb-border)", background: "var(--cb-bg)" }}
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
          <select
            value={theme}
            onChange={async (e) => {
              const t = e.target.value as UiSettings["theme"];
              await ipc.setTheme(t);
              setTheme(t);
            }}
            className="rounded-lg border px-2 py-1 text-sm"
            style={{ borderColor: "var(--cb-border)", background: "var(--cb-bg)" }}
          >
            <option value="system">{t("theme.system")}</option>
            <option value="light">{t("theme.light")}</option>
            <option value="dark">{t("theme.dark")}</option>
          </select>
        </Row>
        <Row label={t("settings.lang")}>
          <select
            value={lang}
            onChange={async (e) => {
              const l = e.target.value as "zh" | "en";
              await ipc.setLang(l).catch((e) => flash(false, String(e)));
              setLangState(l);
            }}
            className="rounded-lg border px-2 py-1 text-sm"
            style={{ borderColor: "var(--cb-border)", background: "var(--cb-bg)" }}
          >
            <option value="zh">简体中文</option>
            <option value="en">English</option>
          </select>
        </Row>
      </Group>

      <Group title={t("settings.systemSection")}>
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
        <Row label={t("settings.snapshot")} hint={t("settings.snapshotAuto")}>
          <span className="text-xs" style={{ color: "var(--cb-text-dim)" }}>{t("settings.snapshotAuto")}</span>
        </Row>
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
            className="rounded-lg border px-3 py-1.5 text-xs"
            style={{ borderColor: "var(--cb-border)" }}
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
    <section className="rounded-xl border"
      style={{ background: "var(--cb-surface)", borderColor: "var(--cb-border)" }}>
      <div className="border-b px-4 py-2.5 text-xs font-medium"
        style={{ borderColor: "var(--cb-border)", color: "var(--cb-text-dim)" }}>
        {title}
      </div>
      <div className="divide-y" style={{ borderColor: "var(--cb-border)" }}>
        {children}
      </div>
    </section>
  );
}

function Row({ label, hint, children }: { label: string; hint?: string; children: React.ReactNode }) {
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
      className="relative h-6 w-11 rounded-full transition-colors"
      style={{ background: checked ? "var(--cb-accent)" : "#8e8e93" }}
    >
      <span
        className="absolute top-0.5 h-5 w-5 rounded-full bg-white transition-all"
        style={{ left: checked ? 22 : 2 }}
      />
    </button>
  );
}
