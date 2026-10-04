import { useCallback, useEffect, useState } from "react";
import { ipc, type GroupView } from "../ipc";
import { useT } from "../i18n";
import { PageHead, ErrorBar } from "./Home";
import { Icon } from "../components/Icon";

/** 代理组页（v2）：组卡片 + 节点网格，点状态灯分色。 */
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
      setGroups((gs) => gs.map((g) => (g.name === group ? { ...g, now: node } : g)));
    } catch (e) {
      setError(String(e));
      load();
    } finally {
      setSelecting(null);
    }
  };

  return (
    <div className="mx-auto max-w-4xl space-y-3.5">
      <PageHead title={t("proxies.title")}>
        <button className="cb-btn acc" onClick={load}>
          <span className="inline-flex items-center gap-1.5">
            <Icon name="refresh" size={12} stroke="currentColor" />
            {t("proxies.refresh")}
          </span>
        </button>
      </PageHead>

      {error && <ErrorBar msg={error} />}

      {groups.length === 0 ? (
        <div
          className="cb-card flex h-64 flex-col items-center justify-center gap-2 text-sm"
          style={{ color: "var(--cb-text-dim)" }}
        >
          <span>{t("proxies.noCore")}</span>
          <span className="text-xs">{error ? String(error) : t("proxies.noCoreHint")}</span>
        </div>
      ) : (
        groups.map((g) => (
          <GroupCard
            key={g.name}
            group={g}
            testing={testing === g.name}
            selecting={selecting}
            onTest={() => testGroup(g.name)}
            onSelect={(n) => select(g.name, n)}
          />
        ))
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
    <section className="cb-card px-[18px] py-4">
      <div className="mb-2.5 flex items-baseline gap-2.5">
        <b className="text-[13px] font-semibold">{group.name}</b>
        <span className="cb-micro">{group.kind}</span>
        <span className="cb-mono ml-auto text-[11px]" style={{ color: "var(--cb-accent)" }}>
          {t("proxies.now")} → {group.now ?? "—"}
        </span>
        <button className="cb-btn" onClick={onTest} disabled={testing}>
          {testing ? t("proxies.testing") : t("proxies.testAll")}
        </button>
      </div>
      <div className="grid grid-cols-6 gap-2">
        {group.nodes.map((n) => {
          const sel = group.now === n.name;
          const cls = delayClass(n.delay_ms);
          return (
            <button
              key={n.name}
              onClick={() => onSelect(n.name)}
              disabled={selecting === `${group.name}:${n.name}`}
              className="rounded-[10px] px-3 py-2.5 text-left disabled:opacity-60"
              style={{
                border: `1px solid ${sel ? "var(--cb-accent-line)" : "var(--cb-line)"}`,
                background: sel
                  ? "linear-gradient(180deg, var(--cb-accent-soft), color-mix(in srgb, var(--cb-accent) 3%, transparent))"
                  : "var(--cb-surface-2)",
              }}
              title={n.kind}
            >
              <div className="truncate text-[12px] font-medium" title={n.name}>
                {n.name}
              </div>
              <div className="mt-1.5 flex items-center gap-1.5">
                <i
                  className="h-[5px] w-[5px] flex-none rounded-full"
                  style={{
                    background:
                      n.delay_ms == null
                        ? "var(--cb-faint)"
                        : cls === "g"
                          ? "var(--cb-ok)"
                          : cls === "y"
                            ? "var(--cb-warn)"
                            : "var(--cb-bad)",
                  }}
                />
                <span className="cb-mono text-[10.5px]" style={{ color: "var(--cb-text-dim)" }}>
                  {delayText(n.delay_ms)}
                </span>
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
  if (ms === 0) return "超时";
  return `${ms} ms`;
}

function delayClass(ms: number | null): "g" | "y" | "r" | "" {
  if (ms == null || ms === 0) return "";
  if (ms <= 200) return "g";
  if (ms <= 500) return "y";
  return "r";
}
