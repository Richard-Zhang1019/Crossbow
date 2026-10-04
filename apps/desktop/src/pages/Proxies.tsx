import { useCallback, useEffect, useState } from "react";
import { ipc, type GroupView } from "../ipc";
import { useT } from "../i18n";

/** 代理页：策略组 + 节点测速/选择（选择由内核 store-selected 持久化）。 */
export default function ProxiesPage() {
  const t = useT();
  const [groups, setGroups] = useState<GroupView[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [testing, setTesting] = useState<string | null>(null);
  const [selecting, setSelecting] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      setGroups(await ipc.proxiesSnapshot());
      setError(null);
    } catch (e) {
      setGroups([]);
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    load();
    const un = ipc.onCoreStatus((s) => {
      if (s.state === "Running") load();
    });
    return () => void un.then((f) => f());
  }, [load]);

  const testGroup = async (name: string) => {
    setTesting(name);
    setError(null);
    try {
      const delays = await ipc.testGroupDelay(name);
      setGroups((gs) =>
        gs.map((g) =>
          g.name !== name
            ? g
            : {
                ...g,
                nodes: g.nodes.map((n) => ({
                  ...n,
                  delay_ms: delays[n.name] ?? null,
                  alive: delays[n.name] !== undefined,
                })),
              },
        ),
      );
    } catch (e) {
      setError(String(e));
    } finally {
      setTesting(null);
    }
  };

  const select = async (group: string, node: string) => {
    setSelecting(`${group}:${node}`);
    setError(null);
    try {
      await ipc.selectProxy(group, node);
      setGroups((gs) =>
        gs.map((g) => (g.name === group ? { ...g, now: node } : g)),
      );
    } catch (e) {
      setError(String(e));
      load();
    } finally {
      setSelecting(null);
    }
  };

  return (
    <div className="mx-auto max-w-4xl space-y-5">
      <div className="flex items-center gap-3">
        <h1 className="text-lg font-semibold">{t("proxies.title")}</h1>
        <span className="text-[11px]" style={{ color: "var(--cb-text-dim)" }}>
          {t("proxies.selectHint")}
        </span>
        <div className="flex-1" />
        {error && (
          <span className="text-xs" style={{ color: "#e0524c" }}>
            {error}
          </span>
        )}
        <button
          onClick={load}
          className="rounded-lg border px-3 py-1.5 text-xs"
          style={{ borderColor: "var(--cb-border)", color: "var(--cb-text-dim)" }}
        >
          {t("proxies.refresh")}
        </button>
      </div>

      {groups.length === 0 ? (
        <div
          className="flex h-64 flex-col items-center justify-center gap-2 rounded-xl border text-sm"
          style={{
            background: "var(--cb-surface)",
            borderColor: "var(--cb-border)",
            color: "var(--cb-text-dim)",
          }}
        >
          <span>{t("proxies.noCore")}</span>
          <span className="text-xs">{error ? String(error) : t("proxies.noCoreHint")}</span>
        </div>
      ) : (
        <div className="space-y-4">
          {groups.map((g) => (
            <GroupCard
              key={g.name}
              group={g}
              testing={testing === g.name}
              selecting={selecting}
              onTest={() => testGroup(g.name)}
              onSelect={(n) => select(g.name, n)}
            />
          ))}
        </div>
      )}
    </div>
  );
}

function GroupCard({
  group,
  testing,
  selecting,
  onTest,
  onSelect,
}: {
  group: GroupView;
  testing: boolean;
  selecting: string | null;
  onTest: () => void;
  onSelect: (node: string) => void;
}) {
  const t = useT();
  return (
    <section className="rounded-xl border p-4"
      style={{ background: "var(--cb-surface)", borderColor: "var(--cb-border)" }}>
      <div className="mb-3 flex items-center gap-2">
        <span className="text-sm font-medium">{group.name}</span>
        <span className="rounded-full px-2 py-0.5 text-[10px]"
          style={{ background: "var(--cb-border)", color: "var(--cb-text-dim)" }}>
          {group.kind}
        </span>
        {group.now && (
          <span className="text-[11px]" style={{ color: "var(--cb-text-dim)" }}>
            {t("proxies.now")}: {group.now}
          </span>
        )}
        <div className="flex-1" />
        <button
          onClick={onTest}
          disabled={testing}
          className="rounded-lg border px-3 py-1 text-xs disabled:opacity-40"
          style={{ borderColor: "var(--cb-border)" }}
        >
          {testing ? t("proxies.testing") : t("proxies.testAll")}
        </button>
      </div>

      <div className="grid grid-cols-4 gap-2">
        {group.nodes.map((n) => {
          const selected = group.now === n.name;
          const busy = selecting === `${group.name}:${n.name}`;
          return (
            <button
              key={n.name}
              onClick={() => onSelect(n.name)}
              disabled={busy}
              className="rounded-lg border p-2.5 text-left transition-colors disabled:opacity-60"
              style={{
                borderColor: selected ? "var(--cb-accent)" : "var(--cb-border)",
                borderWidth: selected ? 2 : 1,
                background: selected ? "var(--cb-bg)" : "transparent",
              }}
              title={n.kind}
            >
              <div className="truncate text-xs font-medium" title={n.name}>
                {n.name}
              </div>
              <div className="mt-1 text-[11px]" style={{ color: delayColor(n.delay_ms) }}>
                {delayText(n.delay_ms)}
              </div>
            </button>
          );
        })}
      </div>
    </section>
  );
}

function delayText(ms: number | null): string {
  if (ms == null) return "—";
  if (ms === 0) return "✕";
  return `${ms}ms`;
}

function delayColor(ms: number | null): string {
  if (ms == null || ms === 0) return "var(--cb-text-dim)";
  if (ms <= 200) return "#34c759";
  if (ms <= 500) return "#d6a02b";
  return "#e0524c";
}
