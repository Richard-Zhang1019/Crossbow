import { useInterval } from "@/hooks/useTimers";
import {ChoiceChip} from "@/components/ChoiceChip";
import {SearchField} from "@/components/Controls";
import PopoverPanel from "@/components/PopoverPanel";
import {friendlyKind} from "@/layouts/layout";
import "./Proxies.css";
import Select from "@/components/Select";
import NodeName from "@/components/NodeName";
import { nodeNameText } from "@/components/NodeName/nodeFlags";
import { notify } from "@/notifications/service";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Button, ButtonBase } from "@/components/Button";
import { ipc, type GroupView, type NodeView } from "@/services/ipc";
import { useT } from "@/i18n/i18n";
import { PageHead } from "@/pages/Home";
import SiteConnectivity from "@/components/SiteConnectivity";
import FitItems from "@/components/FitItems";
import { Icon } from "@/components/Icon";

/** 地区聚类（#141）：旗标优先，名称关键词兜底；顺序即 chips 顺序。 */
const REGIONS: { key: string; flags: string[]; words: string[] }[] = [
  { key: "hk", flags: ["🇭🇰"], words: ["香港", "hong kong", "\\bhk\\b"] },
  { key: "tw", flags: ["🇹🇼"], words: ["台湾", "新北", "彰化", "\\btw\\b"] },
  { key: "jp", flags: ["🇯🇵"], words: ["日本", "东京", "大阪", "\\bjp\\b"] },
  { key: "sg", flags: ["🇸🇬"], words: ["新加坡", "狮城", "\\bsg\\b"] },
  { key: "kr", flags: ["🇰🇷"], words: ["韩国", "首尔", "\\bkr\\b"] },
  {
    key: "us",
    flags: ["🇺🇸"],
    words: ["美国", "洛杉矶", "圣何塞", "西雅图", "纽约", "\\bus\\b"],
  },
  { key: "mo", flags: ["🇲🇴"], words: ["澳门"] },
  {
    key: "eu",
    flags: [
      "🇬🇧",
      "🇫🇷",
      "🇩🇪",
      "🇮🇹",
      "🇳🇱",
      "🇷🇺",
      "🇪🇸",
      "🇵🇱",
      "🇸🇪",
      "🇨🇭",
      "🇦🇹",
      "🇧🇪",
      "🇺🇦",
      "🇨🇿",
    ],
    words: [
      "英国",
      "法国",
      "德国",
      "意大利",
      "荷兰",
      "俄罗斯",
      "西班牙",
      "波兰",
      "瑞典",
      "瑞士",
      "伦敦",
      "法兰克福",
      "\\buk\\b",
      "\\bde\\b",
      "\\bfr\\b",
      "\\bnl\\b",
    ],
  },
];
const OTHER_REGION = "other";

/** 订阅伪节点（流量/到期信息）按名识别，展示为组头小字而非网格卡片 */
const INFO_NAME = /剩余流量|套餐到期|到期时间|官网|重置/;

function regionOf(node: NodeView): string {
  for (const region of REGIONS) {
    if (region.flags.some((f) => node.name.includes(f))) return region.key;
    if (
      region.words.some((w) =>
        w.startsWith("\\")
          ? new RegExp(w, "i").test(node.name)
          : node.name.toLowerCase().includes(w),
      )
    ) {
      return region.key;
    }
  }
  return OTHER_REGION;
}

/** 代理组页（v3，#141）：地区聚类 + 过滤工具栏 + 悬停单节点测速。 */
export default function ProxiesPage() {
  const t = useT();
  const [groupName, setGroupName] = useState("");
  const [groupQuery, setGroupQuery] = useState("");
  const [groups, setGroups] = useState<GroupView[]>([]);
  const [phase, setPhase] = useState<"loading" | "error" | "noConfig" | "stopped" | "ready">("loading");
  const [starting, setStarting] = useState(false);
  const [testing, setTesting] = useState<string | null>(null);
  const [testingNodes, setTestingNodes] = useState<Set<string>>(new Set());
  const batchInFlight = useRef(false);
  const singleInFlight = useRef(new Set<string>());
  const [selecting, setSelecting] = useState<string | null>(null);
  const selectionInFlight = useRef(false);
  const snapshotRevision = useRef(0);

  const load = useCallback(async (silent = false) => {
    const revision = ++snapshotRevision.current;
    if (!silent) setPhase("loading");
    try {
      const [profiles, core] = await Promise.all([ipc.listProfiles(), ipc.coreStatus()]);
      if (revision !== snapshotRevision.current) return;
      if (!profiles.length || core.state !== "Running") {
        setGroups([]); setPhase(!profiles.length ? "noConfig" : "stopped"); return;
      }
      const snapshot = await ipc.proxiesSnapshot();
      if (revision === snapshotRevision.current) { setGroups(snapshot); setPhase("ready"); }
    } catch (e) {
      if (revision === snapshotRevision.current) {
        setPhase("error");
        if (!silent) notify.error(e, "proxies-load");
      }
    }
  }, []);

  useEffect(() => {
    load();
    const un = ipc.onCoreStatus((s) => {
      if (s.state === "Running" && !selectionInFlight.current) void load();
      else if (s.state !== "Running") { ++snapshotRevision.current; setGroups([]); setPhase("stopped"); }
    });
    // 自动组切换与托盘操作发生在页面外，展示状态以最新内核快照为准。
    const switched = () => void load();
    window.addEventListener("cb:profile-switched", switched);
    return () => { window.removeEventListener("cb:profile-switched", switched); ++snapshotRevision.current; void un.then((f) => f()); };
  }, [load]);

  useInterval(() => {
    if (!document.hidden && !selectionInFlight.current) void load(true);
  }, 5000);

  const testGroup = async (name: string) => {
    if (batchInFlight.current || singleInFlight.current.size > 0) return;
    const group = groups.find((g) => g.name === name);
    if (!group) return;

    batchInFlight.current = true;
    setTesting(name);
    setTestingNodes(
      (current) =>
        new Set([
          ...current,
          ...group.nodes.map((node) => nodeTestKey(name, node.name)),
        ]),
    );

    let nextIndex = 0;
    const worker = async () => {
      while (nextIndex < group.nodes.length) {
        const node = group.nodes[nextIndex++];
        try {
          const delay = await ipc.testNodeDelay(node.name);
          setGroups((current) =>
            current.map((g) =>
              g.name !== name
                ? g
                : {
                    ...g,
                    nodes: g.nodes.map((n) =>
                      n.name === node.name
                        ? { ...n, delay_ms: delay, alive: delay > 0 }
                        : n,
                    ),
                  },
            ),
          );
        } catch {
          setGroups((current) =>
            current.map((g) =>
              g.name !== name
                ? g
                : {
                    ...g,
                    nodes: g.nodes.map((n) =>
                      n.name === node.name
                        ? { ...n, delay_ms: 0, alive: false }
                        : n,
                    ),
                  },
            ),
          );
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
      await Promise.all(
        Array.from({ length: Math.min(10, group.nodes.length) }, () =>
          worker(),
        ),
      );
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
      setGroups((current) =>
        current.map((g) =>
          g.name !== group
            ? g
            : {
                ...g,
                nodes: g.nodes.map((n) =>
                  n.name === node
                    ? { ...n, delay_ms: delay, alive: delay > 0 }
                    : n,
                ),
              },
        ),
      );
    } catch {
      setGroups((current) =>
        current.map((g) =>
          g.name !== group
            ? g
            : {
                ...g,
                nodes: g.nodes.map((n) =>
                  n.name === node ? { ...n, delay_ms: 0, alive: false } : n,
                ),
              },
        ),
      );
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
    if (selectionInFlight.current) return;
    selectionInFlight.current = true;
    ++snapshotRevision.current;
    setSelecting(`${group}:${node}`);
    try {
      await notify.run(`proxy-select-${group}`, t("proxies.selectNode"),
        () => ipc.selectProxy(group, node), undefined,
        { feedback: "error-only", titleKey: "proxies.selectNode" });
    } finally {
      await load();
      selectionInFlight.current = false;
      setSelecting(null);
    }
  };

  const restoreAutomatic = async (group: string) => {
    if (selectionInFlight.current) return;
    selectionInFlight.current = true;
    ++snapshotRevision.current;
    setSelecting(`${group}:restore`);
    try {
      await notify.run(`proxy-restore-${group}`, t("proxies.restoreAutomatic"),
        () => ipc.resetProxySelection(group), undefined,
        { feedback: "error-only", titleKey: "proxies.restoreAutomatic" });
    } finally {
      await load();
      selectionInFlight.current = false;
      setSelecting(null);
    }
  };

  const selectedGroup =
    groups.find((group) => group.name === groupName) ?? groups[0];
  const automaticGroup = selectedGroup?.kind === "URLTest" ? selectedGroup
    : groups.find((group) => group.name === selectedGroup?.now && group.kind === "URLTest");
  return (
    <div className="cb-proxies-page cb-node-toolbar-page">
      {(phase !== "ready" || groups.length === 0) && <PageHead title={t("studio.nodes")} meta={t("studio.nodesHint")}>
        <Button disabled={phase === "loading"} onClick={() => void load()}>
          <Icon name="refresh" />
          {t("proxies.refresh")}
        </Button>
      </PageHead>}
      {phase !== "ready" || groups.length === 0 ? (
        <div className="cb-empty" role="status" aria-busy={phase === "loading"}>
          <Icon name="bolt" size={24} />
          <h2>{t(`proxies.state.${phase === "ready" ? "noGroups" : phase}`)}</h2>
          <p>{t(`proxies.hint.${phase === "ready" ? "noGroups" : phase}`)}</p>
          {phase === "noConfig" && <Button onClick={() => window.dispatchEvent(new CustomEvent("cb:goto", { detail: "profiles" }))}>{t("studio.importSource")}</Button>}
          {phase === "stopped" && <Button loading={starting} disabled={starting} onClick={async () => {
            setStarting(true);
            try { await notify.run("nodes-start-core", t("home.core"), () => ipc.coreStart(), undefined, { feedback: "error-only" }); }
            finally { setStarting(false); await load(); }
          }}>{t("proxies.startCore")}</Button>}
          {phase === "error" && <Button onClick={() => void load()}>{t("home.retryState")}</Button>}
        </div>
      ) : (
        <div className="cb-context-layout">
          <nav className="cb-context-nav" aria-label={t("studio.groups")}>
            <h2 className="cb-node-page-title">{t("studio.nodes")}</h2>
            <p className="cb-node-nav-caption">{t("studio.groups")}</p>
            <SearchField className="cb-group-search" label={t("layout.searchGroups")} placeholder={t("layout.searchGroups")} value={groupQuery} onChange={e=>setGroupQuery(e.target.value)}/>
            <div className="cb-context-scroll">
              {groups.filter(group=>group.name.toLowerCase().includes(groupQuery.trim().toLowerCase())).map((group) => (
                <ButtonBase
                  key={group.name}
                  title={`${nodeNameText(group.name)} · ${friendlyKind(group.kind,t)}`}
                  className={selectedGroup?.name === group.name ? "active" : ""}
                  aria-current={
                    selectedGroup?.name === group.name ? "page" : undefined
                  }
                  onClick={() => setGroupName(group.name)}
                >
                  <span><NodeName name={group.name} /></span>
                  <small>{group.kind === "URLTest" && group.fixed ? t("proxies.manuallyFixed") : friendlyKind(group.kind, t)}</small>
                </ButtonBase>
              ))}
              {groupQuery && !groups.some(g=>g.name.toLowerCase().includes(groupQuery.trim().toLowerCase())) && <p className="cb-group-empty">{t("proxies.noMatch")}</p>}
            </div>
          </nav>
          <div className="cb-nodes-workspace">
            {selectedGroup && (
              <GroupCard
                key={selectedGroup.name}
                group={selectedGroup}
                automaticGroup={automaticGroup}
                testing={testing === selectedGroup.name}
                busy={testing !== null}
                testingNodes={testingNodes}
                selecting={selecting}
                onRefresh={() => void load()}
                onRestoreAutomatic={() => automaticGroup && restoreAutomatic(automaticGroup.name)}
                onTest={() => testGroup(selectedGroup.name)}
                onTestNode={(node) => testNode(selectedGroup.name, node)}
                onSelect={(node) => select(selectedGroup.name, node)}
              />
            )}
            <SiteConnectivity
              contextKey={groups
                .map((group) => `${group.name}:${group.now ?? ""}`)
                .join(";")}
            />
          </div>
        </div>
      )}
    </div>
  );
}

function GroupCard({
  group,
  automaticGroup,
  testing,
  busy,
  testingNodes,
  selecting,
  onTest,
  onRefresh,
  onTestNode,
  onSelect,
  onRestoreAutomatic,
}: {
  group: GroupView;
  automaticGroup?: GroupView;
  testing: boolean;
  busy: boolean;
  testingNodes: Set<string>;
  selecting: string | null;
  onTest: () => void;
  onRefresh: () => void;
  onTestNode: (node: string) => void;
  onSelect: (node: string) => void;
  onRestoreAutomatic: () => void;
}) {
  const t = useT();
  const [query, setQuery] = useState("");
  const [region, setRegion] = useState("all");
  const [onlyAlive, setOnlyAlive] = useState(false);
  const [sortByLatency, setSortByLatency] = useState(false);
  const infoNodes = group.nodes.filter((n) => INFO_NAME.test(n.name));
  const realNodes = useMemo(
    () => group.nodes.filter((n) => !INFO_NAME.test(n.name)),
    [group.nodes],
  );

  // 地区 chips：按当前节点集统计（含数量），仅出现有节点的地区
  const regionChips = useMemo(() => {
    const counts = new Map<string, number>();
    for (const n of realNodes) {
      const key = regionOf(n);
      counts.set(key, (counts.get(key) ?? 0) + 1);
    }
    const ordered = REGIONS.filter((r) => counts.has(r.key)).map((r) => ({
      key: r.key,
      count: counts.get(r.key)!,
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
      if (q && !`${n.name} ${n.kind}`.toLowerCase().includes(q)) return false;
      if (onlyAlive && !(n.alive && (n.delay_ms ?? 0) > 0)) return false;
      return true;
    });
    if (sortByLatency) {
      list = [...list].sort((a, b) => {
        const da =
          a.alive && (a.delay_ms ?? 0) > 0
            ? a.delay_ms!
            : Number.MAX_SAFE_INTEGER;
        const db =
          b.alive && (b.delay_ms ?? 0) > 0
            ? b.delay_ms!
            : Number.MAX_SAFE_INTEGER;
        return da - db;
      });
    }
    return list;
  }, [realNodes, region, query, onlyAlive, sortByLatency]);

  const regionLabel = (key: string) =>
    key === OTHER_REGION
      ? t("proxies.regionOther")
      : t(`proxies.region.${key}`);

  return (
    <section className="cb-node-group">
      <div className="cb-node-heading">
        <div className="cb-node-heading-copy">
          <h1><NodeName name={group.name} /><span>{realNodes.length} {t("proxies.nodeCount")}</span></h1>
        </div>
        <div className="cb-node-heading-actions">
          {infoNodes.length > 0 && <PopoverPanel label={t("proxies.subscriptionInfo")} trigger={<Button size="small">{t("proxies.subscriptionInfo")}</Button>}>{()=> <div className="cb-help-copy">{infoNodes.map(n=><p key={n.name}><NodeName name={n.name}/></p>)}</div>}</PopoverPanel>}
          <Button iconOnly className="cb-node-refresh" title={t("proxies.refresh")} aria-label={t("proxies.refresh")} onClick={onRefresh} disabled={busy || selecting !== null}><Icon name="refresh" /></Button>
          <Button variant="primary" className="cb-node-test-all" onClick={onTest} disabled={busy} loading={testing}>
            {testing ? t("proxies.testing") : t("proxies.testAll")}
          </Button>
        </div>
      </div>
      {automaticGroup && <div className="cb-node-automatic" data-fixed={!!automaticGroup.fixed} role="status">
        <div className="cb-node-automatic-copy">
          <strong>{t(automaticGroup.fixed ? "proxies.manuallyFixed" : "proxies.automaticSelection")}</strong>
          <span>{automaticGroup.fixed
            ? <><NodeName name={automaticGroup.fixed} /> · {t("proxies.fixedHint")}</>
            : t("proxies.automaticHint")}</span>
        </div>
        {automaticGroup.fixed && <Button size="small" onClick={onRestoreAutomatic}
          disabled={selecting !== null} loading={selecting === `${automaticGroup.name}:restore`}>
          {t("proxies.restoreAutomatic")}
        </Button>}
      </div>}
      {realNodes.length > 0 && <div className="cb-node-filters">
        <SearchField className="cb-node-search" value={query} onChange={(e) => setQuery(e.target.value)} placeholder={t("proxies.search")} label={t("proxies.search")}/>
        <Select className="cb-node-filter-select" size="compact" label={t("proxies.regionFilter")} value={region} onChange={setRegion}
          options={[{ value: "all", label: t("proxies.allRegions") }, ...regionChips.map((chip) => ({ value: chip.key, label: `${regionLabel(chip.key)} · ${chip.count}` }))]} />
        <Select className="cb-node-filter-select" size="compact" label={t("proxies.sortOrder")} value={sortByLatency ? "latency" : "default"} onChange={(value) => setSortByLatency(value === "latency")}
          options={[{ value: "default", label: t("proxies.defaultOrder") }, { value: "latency", label: t("proxies.latencyOrder") }]} />
        <ChoiceChip selected={onlyAlive} onClick={()=>setOnlyAlive(v=>!v)}>{t("proxies.onlyAlive")}</ChoiceChip>
        <span className="cb-node-result" aria-live="polite">{visible.length} {t("proxies.results")}</span>
      </div>}
      {visible.length === 0 ? <div className="cb-node-filter-empty" role="status"><p>{t("proxies.noMatch")}</p>
        <Button size="small" onClick={() => { setQuery(""); setRegion("all"); setOnlyAlive(false); setSortByLatency(false); }}>{t("proxies.resetFilters")}</Button></div> :
      <div className="cb-node-results">
      <FitItems
        key={`${query}:${region}:${onlyAlive}:${sortByLatency}`}
        items={visible}
        minWidth={200}
        maxColumns={4}
        rowHeight={68}
        maxRowHeight={100}
        maxRows={3}
        label={nodeNameText(group.name)}
        render={(n) => {
          const sel = group.now === n.name;
          const cls = delayClass(n.delay_ms);
          const nodeTesting = testingNodes.has(nodeTestKey(group.name, n.name));
          return (
            <div
              key={n.name}
              role="button"
              tabIndex={0}
              onClick={() => { if (selecting === null) onSelect(n.name); }}
              onKeyDown={(e) => {
                if (
                  e.target === e.currentTarget &&
                  (e.key === "Enter" || e.key === " ")
                ) {
                  e.preventDefault();
                  if (selecting === null) onSelect(n.name);
                }
              }}
              aria-disabled={selecting !== null}
              className={`cb-node-card group relative rounded-[10px] px-3 py-2.5 text-left cursor-pointer transition-colors ${
                selecting === `${group.name}:${n.name}` ? "opacity-60" : ""
              }`}
              style={{
                border: `1px solid ${sel ? "var(--cb-accent-line)" : "var(--cb-line)"}`,
                background: sel
                  ? "var(--cb-accent-soft)"
                  : "var(--cb-node-surface)",
              }}
              title={group.kind === "URLTest" ? t("proxies.pinHint") : n.kind}
            >
              {sel && !nodeTesting && (
                <span className="cb-node-selected">
                  <Icon name="check" size={17} strokeWidth={2} />
                </span>
              )}
              <Button iconOnly size="small"
                className="cb-node-test z-10 opacity-0 group-hover:opacity-100 group-focus-within:opacity-100"
                title={`${t("proxies.testNode")} ${nodeNameText(n.name)}`}
                aria-label={`${t("proxies.testNode")} ${nodeNameText(n.name)}`}
                onClick={(e) => {
                  e.stopPropagation();
                  onTestNode(n.name);
                }}
                style={{ opacity: nodeTesting ? 1 : undefined }}
                disabled={nodeTesting || busy} loading={nodeTesting}
              >
                <Icon name="bolt" size={13} />
              </Button>
              <div
                className="cb-node-title pointer-events-none truncate text-[13px] font-medium"
                title={nodeNameText(n.name)}
              >
                <NodeName name={n.name} />
              </div>
              <div className="cb-node-bottom pointer-events-none mt-1.5 flex items-center gap-1.5">
                <span className="cb-node-kind" title={n.kind}>{friendlyKind(n.kind,t)}</span>
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
                  style={{
                    color: nodeTesting
                      ? "var(--cb-accent)"
                      : "var(--cb-text-dim)",
                  }}
                >
                  {nodeTesting ? t("proxies.testing") : delayText(n.delay_ms, t)}
                </span>
              </div>
            </div>
          );
        }}
      /></div>}
    </section>
  );
}

function nodeTestKey(group: string, node: string): string {
  return `${group}\u0000${node}`;
}

function delayText(ms: number | null, t: ReturnType<typeof useT>): string {
  if (ms == null) return "—";
  if (ms === 0) return t("proxies.timeout");
  return `${ms} ms`;
}

function delayClass(ms: number | null): "g" | "y" | "r" | "" {
  if (ms == null || ms === 0) return "";
  if (ms <= 200) return "g";
  if (ms <= 500) return "y";
  return "r";
}
