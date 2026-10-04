import { useEffect, useState } from "react";
import { ipc, type EngineConfigView, type UiSettings } from "../ipc";

/** 设置页（M0-S4）：内核端口/局域网、外观主题、开机自启、内核版本。 */
export default function SettingsPage() {
  const [engine, setEngine] = useState<EngineConfigView | null>(null);
  const [portDraft, setPortDraft] = useState<string>("");
  const [theme, setTheme] = useState<UiSettings["theme"]>("system");
  const [autostart, setAutostart] = useState(false);
  const [version, setVersion] = useState<string>("…");
  const [msg, setMsg] = useState<{ ok: boolean; text: string } | null>(null);

  const load = async () => {
    const e = await ipc.getEngineConfig().catch(() => null);
    setEngine(e);
    setPortDraft(e ? String(e.mixed_port) : "");
    setTheme(await ipc.getUiSettings().then((u) => u.theme).catch(() => "system" as const));
    setAutostart(await ipc.autostartStatus().catch(() => false));
    setVersion(await ipc.coreVersion().catch(() => "未安装"));
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
      flash(false, "端口需为 1024–65535 的整数");
      return;
    }
    try {
      await ipc.setMixedPort(port);
      flash(true, `混合端口已改为 ${port}${engine?.allow_lan ? "" : "（仅本机监听）"}`);
    } catch (e) {
      flash(false, String(e));
    }
  };

  return (
    <div className="mx-auto max-w-3xl space-y-5">
      <div className="flex items-center justify-between">
        <h1 className="text-lg font-semibold">设置</h1>
        {msg && (
          <span className="text-xs" style={{ color: msg.ok ? "#34c759" : "#e0524c" }}>
            {msg.text}
          </span>
        )}
      </div>

      <Group title="内核">
        <Row label="内核版本" hint="mihomo sidecar">
          <span className="cb-selectable text-xs" style={{ color: "var(--cb-text-dim)" }}>
            {version}
          </span>
        </Row>
        <Row label="混合监听端口" hint="HTTP + SOCKS5 共用；修改后内核运行中会自动热重启">
          <input
            value={portDraft}
            onChange={(e) => setPortDraft(e.target.value)}
            onBlur={applyPort}
            onKeyDown={(e) => e.key === "Enter" && applyPort()}
            className="cb-selectable w-24 rounded-lg border px-2 py-1 text-sm outline-none focus:border-[var(--cb-accent)]"
            style={{ borderColor: "var(--cb-border)", background: "var(--cb-bg)" }}
          />
        </Row>
        <Row label="允许局域网连接" hint="同一网络下的其他设备可使用本机代理（bind 0.0.0.0）">
          <Switch
            checked={engine?.allow_lan ?? false}
            onChange={async (v) => {
              await ipc.setAllowLan(v).catch((e) => flash(false, String(e)));
              load();
              flash(true, v ? "已允许局域网" : "已改为仅本机");
            }}
          />
        </Row>
      </Group>

      <Group title="外观">
        <Row label="主题" hint="跟随系统 / 浅色 / 深色">
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
            <option value="system">跟随系统</option>
            <option value="light">浅色</option>
            <option value="dark">深色</option>
          </select>
        </Row>
        <Row label="语言" hint="英文界面在 M1 提供">
          <select disabled className="rounded-lg border px-2 py-1 text-sm opacity-50"
            style={{ borderColor: "var(--cb-border)", background: "var(--cb-bg)" }}>
            <option>简体中文</option>
            <option>English</option>
          </select>
        </Row>
      </Group>

      <Group title="系统">
        <Row label="开机自启" hint="登录后自动启动（隐藏窗口，驻留托盘）">
          <Switch
            checked={autostart}
            onChange={async (v) => {
              const ok = await ipc.autostartSet(v).then(() => true).catch((e) => {
                flash(false, String(e));
                return false;
              });
              if (ok) {
                setAutostart(v);
                flash(true, v ? "已开启自启" : "已关闭自启");
              }
            }}
          />
        </Row>
        <Row label="配置快照" hint="每次保存前自动快照，保留最近 5 份">
          <span className="text-xs" style={{ color: "var(--cb-text-dim)" }}>自动</span>
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
