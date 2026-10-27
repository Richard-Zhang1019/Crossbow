import { notify } from "../notifications/service";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { ipc, type GroupView, type NodeView } from "../ipc";
import { useT } from "../i18n";
import { PageHead } from "./Home";
import SiteConnectivity from "../components/SiteConnectivity";
import { Icon } from "../components/Icon";

/** 地区聚类（#141）：旗标优先，名称关键词兜底；顺序即 chips 顺序。 */
const REGIONS: { key: string; flags: string[]; words: string[] }[] = [
  { key: "hk", flags: ["🇭🇰"], words: ["香港", "hong kong", "\\bhk\\b"] },
  { key: "tw", flags: ["🇹🇼"], words: ["台湾", "新北", "彰化", "\\btw\\b"] },
  { key: "jp", flags: ["🇯🇵"], words: ["日本", "东京", "大阪", "\\bjp\\b"] },
  { key: "sg", flags: ["🇸🇬"], words: ["新加坡", "狮城", "\\bsg\\b"] },
  { key: "kr", flags: ["🇰🇷"], words: ["韩国", "首尔", "\\bkr\\b"] },
  { key: "us", flags: ["🇺🇸"], words: ["美国", "洛杉矶", "圣何塞", "西雅图", "纽约", "\\bus\\b"] },
  { key: "mo", flags: ["🇲🇴"], words: ["澳门"] },
  {
    key: "eu",
    flags: ["🇬🇧", "🇫🇷", "🇩🇪", "🇮🇹", "🇳🇱", "🇷🇺", "🇪🇸", "🇵🇱", "🇸🇪", "🇨🇭", "🇦🇹", "🇧🇪", "🇺🇦", "🇨🇿"],
    words: ["英国", "法国", "德国", "意大利", "荷兰", "俄罗斯", "西班牙", "波兰", "瑞典", "瑞士", "伦敦", "法兰克福", "\\buk\\b", "\\bde\\b", "\\bfr\\b", "\\bnl\\b"],
  },
];
const OTHER_REGION = "other";

/** 订阅伪节点（流量/到期信息）按名识别，展示为组头小字而非网格卡片 */
const INFO_NAME = /剩余流量|套餐到期|到期时间|官网|重置/;

function regionOf(node: NodeView): string {
  for (const region of REGIONS) {
    if (region.flags.some((f) => node.name.includes(f))) return region.key;
    if (region.words.some((w) => (w.startsWith("\\") ? new RegExp(w, "i").test(node.name) : node.name.toLowerCase().includes(w)))) {
      return region.key;
    }
  }
  return OTHER_REGION;
}

/** 代理组页（v3，#141）：地区聚类 + 过滤工具栏 + 悬停单节点测速。 */
export default function ProxiesPage() {
  const t = useT();
  const [groups, setGroups] = useState<GroupView[]>([]);
  const [testing, setTesting] = useState<string | null>(null);
  const [testingNodes, setTestingNodes] = useState<Set<string>>(new Set());
  const batchInFlight = useRef(false);
  const singleInFlight = useRef(new Set<string>());
  const [selecting, setSelecting] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      setGroups(await ipc.proxiesSnapshot());
    } catch (e) {
      setGroups([]);
      notify.error(e, "proxies-load");
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
    if (batchInFlight.current || singleInFlight.current.size > 0) return;
    const group = groups.find((g) => g.name === name);
    if (!group) return;

    batchInFlight.current = true;
    setTesting(name);
    setTestingNodes((current) => new Set([
      ...current,
      ...group.nodes.map((node) => nodeTestKey(name, node.name)),
    ]));

    let nextIndex = 0;
    const worker = async () => {
      while (nextIndex < group.nodes.length) {
        const node = group.nodes[nextIndex++];
        try {
          const delay = await ipc.testNodeDelay(node.name);
          setGroups((current) => current.map((g) =>
            g.name !== name ? g : {
              ...g,
              nodes: g.nodes.map((n) => n.name === node.name
                ? { ...n, delay_ms: delay, alive: delay > 0 }
                : n),
            },
          ));
        } catch {
          setGroups((current) => current.map((g) =>
            g.name !== name ? g : {
              ...g,
              nodes: g.nodes.map((n) => n.name === node.name
                ? { ...n, delay_ms: 0, alive: false }
                : n),
            },
          ));
        } finally {
          setTestingNodes((current) => {
            const next = new Set(current);
            next.delete(nodeTestKey(name, node.name));
            return next;
          });
        }
      }
    };

    try {
      await Promise.all(Array.from(
        { length: Math.min(10, group.nodes.length) },
        () => worker(),
      ));
    } finally {
      // 完成不发任何通知（#140）：延迟值原位刷新即反馈
      batchInFlight.current = false;
      setTesting(null);
    }
  };

  const testNode = async (group: string, node: string) => {
    const key = nodeTestKey(group, node);
    if (testingNodes.has(key) || testing) return;
    singleInFlight.current.add(node);
    setTestingNodes((current) => new Set([...current, key]));
    try {
      const delay = await ipc.testNodeDelay(node);
      setGroups((current) => current.map((g) =>
        g.name !== group ? g : {
          ...g,
          nodes: g.nodes.map((n) => n.name === node
            ? { ...n, delay_ms: delay, alive: delay > 0 }
            : n),
        },
      ));
    } catch {
      setGroups((current) => current.map((g) =>
        g.name !== group ? g : {
          ...g,
          nodes: g.nodes.map((n) => n.name === node
            ? { ...n, delay_ms: 0, alive: false }
            : n),
        },
      ));
    } finally {
      singleInFlight.current.delete(node);
      setTestingNodes((current) => {
        const next = new Set(current);
        next.delete(key);
        return next;
      });
    }
  };

  const select = async (group: string, node: string) => {
    setSelecting(`${group}:${node}`);
    try {
      await ipc.selectProxy(group, node);
      setGroups((gs) => gs.map((g) => (g.name === group ? { ...g, now: node } : g)));
    } catch (e) {
      notify.error(e, `proxy-select-${group}`);
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

      <SiteConnectivity contextKey={groups.map((group) => `${group.name}:${group.now ?? ""}`).join(";")} />
      {groups.length === 0 ? (
        <div
          className="cb-card flex h-64 flex-col items-center justify-center gap-2 text-sm"
          style={{ color: "var(--cb-text-dim)" }}
        >
          <span>{t("proxies.noCore")}</span>
          <span className="text-xs">{t("proxies.noCoreHint")}</span>
        </div>
      ) : (
        groups.map((g) => (
          <GroupCard
            key={g.name}
            group={g}
            testing={testing === g.name}
            busy={testing !== null}
            testingNodes={testingNodes}
            selecting={selecting}
            onTest={() => testGroup(g.name)}
            onTestNode={(n) => testNode(g.name, n)}
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
  busy,
  testingNodes,
  selecting,
  onTest,
  onTestNode,
  onSelect,
}: {
  group: GroupView;
  testing: boolean;
  busy: boolean;
  testingNodes: Set<string>;
  selecting: string | null;
  onTest: () => void;
  onTestNode: (node: string) => void;
  onSelect: (node: string) => void;
}) {
  const t = useT();
  const [query, setQuery] = useState("");
  const [region, setRegion] = useState("all");
  const [onlyAlive, setOnlyAlive] = useState(false);
  const [sortByLatency, setSortByLatency] = useState(false);

  const infoNodes = group.nodes.filter((n) => INFO_NAME.test(n.name));
  const realNodes = useMemo(() => group.nodes.filter((n) => !INFO_NAME.test(n.name)), [group.nodes]);

  // 地区 chips：按当前节点集统计（含数量），仅出现有节点的地区
  const regionChips = useMemo(() => {
    const counts = new Map<string, number>();
    for (const n of realNodes) {
      const key = regionOf(n);
      counts.set(key, (counts.get(key) ?? 0) + 1);
    }
    const ordered = REGIONS.filter((r) => counts.has(r.key)).map((r) => ({
      key: r.key, count: counts.get(r.key)!,
    }));
    if (counts.has(OTHER_REGION)) {
      ordered.push({ key: OTHER_REGION, count: counts.get(OTHER_REGION)! });
    }
    return ordered;
  }, [realNodes]);

  const visible = useMemo(() => {
    const q = query.trim().toLowerCase();
    let list = realNodes.filter((n) => {
      if (region !== "all" && regionOf(n) !== region) return false;
      if (q && !n.name.toLowerCase().includes(q)) return false;
      if (onlyAlive && !(n.alive && (n.delay_ms ?? 0) > 0)) return false;
      return true;
    });
    if (sortByLatency) {
      list = [...list].sort((a, b) => {
        const da = a.alive && (a.delay_ms ?? 0) > 0 ? a.delay_ms! : Number.MAX_SAFE_INTEGER;
        const db = b.alive && (b.delay_ms ?? 0) > 0 ? b.delay_ms! : Number.MAX_SAFE_INTEGER;
        return da - db;
      });
    }
    return list;
  }, [realNodes, region, query, onlyAlive, sortByLatency]);

  const regionLabel = (key: string) =>
    key === OTHER_REGION ? t("proxies.regionOther") : t(`proxies.region.${key}`);

  return (
    <section className="cb-card px-[18px] py-4">
      <div className="mb-2.5 flex items-baseline gap-2.5">
        <b className="text-[13px] font-semibold">{group.name}</b>
        <span className="cb-micro">{group.kind}</span>
        <span className="cb-mono ml-auto text-[11px]" style={{ color: "var(--cb-accent)" }}>
          {t("proxies.now")} → {group.now ?? "—"}
        </span>
        <button
          className={`cb-btn flex items-center gap-1.5 ${testing ? "opacity-60" : ""}`}
          onClick={onTest}
          disabled={busy}
        >
          <svg
            className={`h-3 w-3 ${testing ? "animate-spin" : ""}`}
            viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2"
          >
            <path d="M21 12a9 9 0 1 1-9-9" strokeLinecap="round" />
          </svg>
          {testing ? t("proxies.testing") : t("proxies.testAll")}
        </button>
      </div>

      {infoNodes.length > 0 && (
        <div className="mb-2.5 flex flex-wrap gap-1.5">
          {infoNodes.map((n) => (
            <span
              key={n.name}
              className="cb-mono rounded-md px-2 py-0.5 text-[10.5px]"
              style={{ background: "var(--cb-surface-2)", color: "var(--cb-text-dim)" }}
            >
              {n.name}
              {n.alive && (n.delay_ms ?? 0) > 0 ? ` · ${n.delay_ms}ms` : ""}
            </span>
          ))}
        </div>
      )}

      {realNodes.length > 8 && (
        <div className="mb-2.5 flex flex-wrap items-center gap-1.5">
          <input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder={t("proxies.search")}
            className="h-7 w-36 rounded-lg border border-[var(--cb-line)] bg-[var(--cb-surface-2)] px-2.5 text-[12px] outline-none placeholder:text-[var(--cb-faint)] focus:border-[var(--cb-accent-line)]"
          />
          <button
            className={`rounded-full px-2.5 py-1 text-[11px] transition-colors ${
              region === "all" ? "text-[var(--cb-accent)]" : "text-[var(--cb-text-dim)]"
            }`}
            style={{
              background: region === "all" ? "var(--cb-accent-soft)" : "var(--cb-surface-2)",
            }}
            onClick={() => setRegion("all")}
          >
            {t("proxies.all")} {realNodes.length}
          </button>
          {regionChips.map((chip) => (
            <button
              key={chip.key}
              className={`rounded-full px-2.5 py-1 text-[11px] transition-colors ${
                region === chip.key ? "text-[var(--cb-accent)]" : "text-[var(--cb-text-dim)]"
              }`}
              style={{
                background: region === chip.key ? "var(--cb-accent-soft)" : "var(--cb-surface-2)",
              }}
              onClick={() => setRegion(region === chip.key ? "all" : chip.key)}
            >
              {regionLabel(chip.key)} {chip.count}
            </button>
          ))}
          <span className="grow" />
          <button
            className={`cb-btn h-7 px-2 text-[11px] ${onlyAlive ? "text-[var(--cb-accent)]" : ""}`}
            onClick={() => setOnlyAlive((v) => !v)}
          >
            {t("proxies.onlyAlive")}
          </button>
          <button
            className={`cb-btn h-7 px-2 text-[11px] ${sortByLatency ? "text-[var(--cb-accent)]" : ""}`}
            onClick={() => setSortByLatency((v) => !v)}
          >
            {t("proxies.sortLatency")}
          </button>
        </div>
      )}

      <div className="grid grid-cols-6 gap-2">
        {visible.map((n) => {
          const sel = group.now === n.name;
          const cls = delayClass(n.delay_ms);
          const nodeTesting = testingNodes.has(nodeTestKey(group.name, n.name));
          return (
            <div
              key={n.name}
              role="button"
              tabIndex={0}
              onClick={() => onSelect(n.name)}
              onKeyDown={(e) => {
                if (e.key === "Enter" || e.key === " ") onSelect(n.name);
              }}
              aria-disabled={selecting === `${group.name}:${n.name}`}
              className={`group relative rounded-[10px] px-3 py-2.5 text-left cursor-pointer transition-colors ${
                selecting === `${group.name}:${n.name}` ? "opacity-60" : ""
              } hover:border-[var(--cb-accent-line)]`}
              style={{
                border: `1px solid ${sel ? "var(--cb-accent-line)" : "var(--cb-line)"}`,
                background: sel
                  ? "linear-gradient(180deg, var(--cb-accent-soft), color-mix(in srgb, var(--cb-accent) 3%, transparent))"
                  : "var(--cb-surface-2)",
              }}
              title={n.kind}
            >
              <button
                className="absolute right-1.5 top-1.5 z-10 flex h-5 w-5 items-center justify-center rounded-md border border-[var(--cb-line)] bg-[var(--cb-surface)] text-[10px] opacity-0 transition-opacity hover:text-[var(--cb-accent)] group-hover:opacity-100"
                title={`${t("proxies.testNode")} ${n.name}`}
                aria-label={`${t("proxies.testNode")} ${n.name}`}
                onClick={(e) => {
                  e.stopPropagation();
                  onTestNode(n.name);
                }}
                disabled={nodeTesting || busy}
              >
                {nodeTesting ? (
                  <svg
                    className="h-2.5 w-2.5 animate-spin"
                    viewBox="0 0 24 24" fill="none" stroke="var(--cb-accent)" strokeWidth="2.5"
                    aria-label={t("proxies.testing")}
                  >
                    <path d="M21 12a9 9 0 1 1-9-9" strokeLinecap="round" />
                  </svg>
                ) : "⚡"}
              </button>
              <div className="pointer-events-none truncate pr-5 text-[12px] font-medium" title={n.name}>
                {n.name}
              </div>
              <div className="pointer-events-none mt-1.5 flex items-center gap-1.5">
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
                <span
                  className="cb-mono text-[10.5px]"
                  style={{ color: nodeTesting ? "var(--cb-accent)" : "var(--cb-text-dim)" }}
                >
                  {nodeTesting ? t("proxies.testing") : delayText(n.delay_ms)}
                </span>
              </div>
            </div>
          );
        })}
      </div>
      {visible.length === 0 && realNodes.length > 0 && (
        <div className="py-6 text-center text-xs" style={{ color: "var(--cb-faint)" }}>
          {t("proxies.noMatch")}
        </div>
      )}
    </section>
  );
}

function nodeTestKey(group: string, node: string): string {
  return `${group}\u0000${node}`;
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
