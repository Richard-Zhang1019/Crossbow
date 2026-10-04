import { useCallback, useEffect, useState } from "react";
import {
  ipc,
  type CoreStatus,
  type Profile,
} from "../ipc";

/** 首页仪表盘（M0 精简版）：内核开关 + 状态 + 配置概览。 */
export default function HomePage() {
  const [profiles, setProfiles] = useState<Profile[]>([]);
  const [activeId, setActiveId] = useState<string | null>(null);
  const [status, setStatus] = useState<CoreStatus>({ state: "Stopped" });
  const [busy, setBusy] = useState(false);

  const refresh = useCallback(async () => {
    setProfiles(await ipc.listProfiles().catch(() => []));
    setActiveId(await ipc.activeProfileId().catch(() => null));
    setStatus(await ipc.coreStatus().catch(() => ({ state: "Stopped" as const })));
  }, []);

  useEffect(() => {
    refresh();
    const un = ipc.onCoreStatus(setStatus);
    return () => void un.then((f) => f());
  }, [refresh]);

  const toggle = async () => {
    setBusy(true);
    try {
      if (status.state === "Running" || status.state === "Starting") {
        await ipc.coreStop();
      } else {
        await ipc.coreStart();
      }
    } catch (e) {
      console.error(e);
    } finally {
      setBusy(false);
      refresh();
    }
  };

  const on = status.state === "Running" || status.state === "Starting";
  const active = profiles.find((p) => p.id === activeId);

  return (
    <div className="mx-auto max-w-4xl space-y-5">
      <h1 className="text-lg font-semibold">首页</h1>

      <section className="rounded-xl border p-5"
        style={{ background: "var(--cb-surface)", borderColor: "var(--cb-border)" }}>
        <div className="flex items-center justify-between">
          <div>
            <div className="text-sm font-medium">内核（mihomo sidecar）</div>
            <div className="mt-0.5 text-xs" style={{ color: "var(--cb-text-dim)" }}>
              {statusText(status)}
            </div>
          </div>
          <button
            onClick={toggle}
            disabled={busy || !activeId}
            title={activeId ? undefined : "请先在「配置」页导入订阅"}
            className="rounded-full px-4 py-1.5 text-sm font-medium text-white disabled:opacity-40"
            style={{ background: on ? "var(--cb-accent)" : "#8e8e93" }}
          >
            {busy ? "…" : on ? "停止" : "启动"}
          </button>
        </div>
      </section>

      <div className="grid grid-cols-3 gap-4">
        <Card title="当前配置">
          <div className="text-sm">{active ? active.name : "— 未选择 —"}</div>
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

function statusText(s: CoreStatus): string {
  switch (s.state) {
    case "Running":
      return "运行中";
    case "Starting":
      return "启动中…";
    case "Crashed":
      return `异常：${s.message ?? "未知错误"}`;
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
