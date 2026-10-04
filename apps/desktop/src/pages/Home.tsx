import { useCallback, useEffect, useState } from "react";
import {
  CORE_MODES,
  ipc,
  type CoreMode,
  type CoreStatus,
  type Profile,
  type SysProxyStatus,
} from "../ipc";

/** 首页仪表盘（M0-S3）：系统代理总开关 + 运行模式 + 配置概览。 */
export default function HomePage() {
  const [profiles, setProfiles] = useState<Profile[]>([]);
  const [activeId, setActiveId] = useState<string | null>(null);
  const [core, setCore] = useState<CoreStatus>({ state: "Stopped" });
  const [proxy, setProxy] = useState<SysProxyStatus>({ enabled: false, port: 0 });
  const [mode, setMode] = useState<CoreMode>("rule");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    setProfiles(await ipc.listProfiles().catch(() => []));
    setActiveId(await ipc.activeProfileId().catch(() => null));
    setCore(await ipc.coreStatus().catch(() => ({ state: "Stopped" as const })));
    setProxy(await ipc.sysproxyStatus().catch(() => ({ enabled: false, port: 0 })));
    setMode(await ipc.coreMode().catch(() => "rule" as const));
  }, []);

  useEffect(() => {
    refresh();
    const un1 = ipc.onCoreStatus(setCore);
    const un2 = ipc.onSysproxyStatus(setProxy);
    const un3 = ipc.onCoreMode(setMode);
    return () => {
      for (const un of [un1, un2, un3]) void un.then((f) => f());
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

  return (
    <div className="mx-auto max-w-4xl space-y-5">
      <h1 className="text-lg font-semibold">首页</h1>

      <section className="rounded-xl border p-5"
        style={{ background: "var(--cb-surface)", borderColor: "var(--cb-border)" }}>
        <div className="flex items-center justify-between">
          <div>
            <div className="text-sm font-medium">
              系统代理{proxy.enabled ? ` :${proxy.port}` : ""}
            </div>
            <div className="mt-0.5 text-xs" style={{ color: "var(--cb-text-dim)" }}>
              内核 {coreStatusText(core)} · 当前配置 {active ? active.name : "未选择"}
            </div>
          </div>
          <button
            onClick={toggleProxy}
            disabled={busy || !activeId}
            title={activeId ? undefined : "请先在「配置」页导入订阅"}
            className="rounded-full px-4 py-1.5 text-sm font-medium text-white disabled:opacity-40"
            style={{ background: proxy.enabled ? "var(--cb-accent)" : "#8e8e93" }}
          >
            {busy ? "…" : proxy.enabled ? "关闭" : "开启"}
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
              {m.label}
            </button>
          ))}
          <span className="ml-2 text-[11px]" style={{ color: "var(--cb-text-dim)" }}>
            内核运行中才实际生效
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
        <Card title="当前出口">
          <div className="text-sm">— 未连接 —</div>
        </Card>
        <Card title="今日流量">
          <div className="text-sm">↑ 0 B　↓ 0 B</div>
        </Card>
        <Card title="活动连接">
          <div className="text-sm">0</div>
        </Card>
      </div>

      <Card title="配置档案">
        {profiles.length === 0 ? (
          <div className="text-xs" style={{ color: "var(--cb-text-dim)" }}>
            暂无配置 — 到「配置」页导入第一个订阅
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

function coreStatusText(s: CoreStatus): string {
  switch (s.state) {
    case "Running":
      return "运行中";
    case "Starting":
      return "启动中…";
    case "Crashed":
      return `异常（${s.message ?? "未知"}）`;
    default:
      return "已停止";
  }
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
