import SegmentedControl from "@/components/SegmentedControl";
import {SearchField} from "@/components/Controls";
import Dialog from "@/components/Dialog";
import Select from "@/components/Select";
import { sortConnections, connectionDetails } from "@/lib/workflow";
import { ButtonBase, Button } from "@/components/Button";
import NodeName from "@/components/NodeName";
import { nodeNameText } from "@/components/NodeName/nodeFlags";
import { notify } from "@/notifications/service";
import { useEffect, useMemo, useRef, useState } from "react";
import VirtualList from "@/components/VirtualList";
import { fmtBytes, fmtDuration } from "@/lib/format";
import { ipc, type ConnRow, type ConnSnapshot } from "@/services/ipc";
import { useT } from "@/i18n/i18n";
import { PageHead } from "@/pages/Home";

const ROW_H = 48;
const CONNECTION_COLUMNS =
  "minmax(190px,2fr) 100px minmax(160px,1.2fr) 120px 80px 80px 60px 24px";
const PROCESS_COLUMNS = "minmax(200px,1.6fr) 90px 100px 100px 90px";

/** 连接页：列表 / 按进程聚合双视图（内核 connections WS → Rust 节流聚合）。 */
export default function ConnectionsPage() {
  const t = useT();
  const [snapshot, setSnapshot] = useState<ConnSnapshot>({
    rows: [],
    upload_total: null,
    download_total: null,
    memory: null,
  });
  const [inspected, setInspected] = useState<ConnRow | null>(null);
  const [sort, setSort] = useState("default");
  const [copied, setCopied] = useState(false);
  const [filter, setFilter] = useState("");
  const [paused, setPaused] = useState(false);
  const [view, setView] = useState<"list" | "apps">("list");
  const latest = useRef<ConnSnapshot>(snapshot);

  useEffect(() => {
    ipc
      .subscribeConnections()
      .catch((error) => notify.error(error, "connections-stream"));
    const un = ipc.onConnections((s) => {
      latest.current = s;
      if (!paused) setSnapshot(s);
    });
    return () => {
      void un.then((f) => f());
      ipc.unsubscribeConnections().catch(() => {});
    };
  }, [paused]);

  const closeOne = async (id: string) => {
    try {
      await ipc.closeConnection(id);
      // 乐观移除，等待下个快照校正
      setSnapshot((s) => ({ ...s, rows: s.rows.filter((r) => r.id !== id) }));
    } catch (e) {
      notify.error(e, "connections-operation");
    }
  };

  const togglePause = () => {
    if (paused) {
      // 恢复时立刻补上暂停期间的数据
      setSnapshot(latest.current);
    }
    setPaused(!paused);
  };

  const rows = useMemo(() => {
    const q = filter.trim().toLowerCase();
    const matching = !q ? snapshot.rows : snapshot.rows.filter((r) =>
      [r.host, r.rule, r.chains, r.process].some((f) =>
        f.toLowerCase().includes(q),
      ),
    );
    return sortConnections(matching, sort);
  }, [snapshot, filter, sort]);

  const apps = useMemo(() => aggregateByProcess(rows), [rows]);
  const selected = snapshot.rows.find(r => r.id === inspected?.id) ?? inspected;
  const copyDetails = async (value:string) => {try {await navigator.clipboard.writeText(value); setCopied(true); window.setTimeout(()=>setCopied(false),1500);}catch(e){notify.error(e,"connection-copy");}};

  return (
    <div className="cb-connections-page">
      <PageHead
        title={t("conn.title")}
        meta={`${snapshot.truncated ? t("conn.rowCountTruncated", {n: snapshot.truncated, shown: rows.length}) : t("conn.rowCount", {n: rows.length})} · ↑${fmtBytes(snapshot.upload_total)} ↓${fmtBytes(snapshot.download_total)} · ${t("conn.mem")} ${fmtBytes(snapshot.memory)}`}
      />
      <div className="cb-connection-toolbar">
        <SegmentedControl label={t("conn.title")} value={view} onChange={setView} options={[{value:"list",label:t("conn.viewList")},{value:"apps",label:t("conn.viewApps")}]} />
        <SearchField label={t("conn.filter")}
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
          placeholder={t("conn.filter")}
          className="cb-connection-search"
        />
        <Select label={t("conn.sort")} value={sort} onChange={setSort} options={["default","down","up","age"].map(value=>({value,label:t(`conn.sort.${value}`)}))} />
        <Button aria-pressed={paused} onClick={togglePause}>
          {paused ? t("conn.paused") : t("conn.pause")}
        </Button>
      </div>

      <div className="cb-card cb-connection-table">
        {view === "list" ? (
          <VirtualList
            items={rows}
            rowHeight={ROW_H}
            minWidth={1010}
            header={
              <div
                className="cb-th cb-table-row"
                style={{ gridTemplateColumns: CONNECTION_COLUMNS }}
              >
                <span>{t("col.target")}</span>
                <span>{t("col.rule")}</span>
                <span>{t("col.chains")}</span>
                <span>{t("col.process")}</span>
                <span className="text-right">{t("col.up")}</span>
                <span className="text-right">{t("col.down")}</span>
                <span className="text-right">{t("col.elapsed")}</span>
                <span />
              </div>
            }
            empty={
              <Empty
                text={
                  snapshot.rows.length === 0
                    ? t("conn.empty")
                    : t("conn.noMatch")
                }
              />
            }
            render={(row: ConnRow) => (
              <Row key={row.id} row={row} onInspect={() => {setCopied(false); setInspected(row);}} onClose={() => closeOne(row.id)} />
            )}
          />
        ) : (
          <VirtualList
            items={apps}
            rowHeight={44}
            minWidth={650}
            header={
              <div
                className="cb-th cb-table-row"
                style={{ gridTemplateColumns: PROCESS_COLUMNS }}
              >
                <span>{t("col.process")}</span>
                <span className="text-right">{t("conn.connCount")}</span>
                <span className="text-right">{t("col.up")}</span>
                <span className="text-right">{t("col.down")}</span>
                <span className="text-right">{t("col.elapsed")}</span>
              </div>
            }
            empty={
              <Empty
                text={
                  snapshot.rows.length === 0
                    ? t("conn.empty")
                    : t("conn.noMatch")
                }
              />
            }
            render={(app: ProcessAgg) => (
              <AppRow
                key={app.process}
                agg={app}
                onInspect={() => {
                  setFilter(app.process);
                  setView("list");
                }}
              />
            )}
          />
        )}
      </div>
      {selected && <Dialog title={t("conn.details")} onClose={()=>setInspected(null)}>
        {!snapshot.rows.some(r=>r.id===selected.id) && <p className="cb-control-note">{t("conn.closedDetail")}</p>}
        <dl className="cb-connection-details">{Object.entries(connectionDetails(selected)).map(([key,value])=><div key={key}><dt>{t(({Target:"col.target",Network:"workflow.network",Type:"workflow.type",Rule:"col.rule",Chain:"col.chains",Process:"col.process",ID:"workflow.id"} as Record<string,string>)[key])}</dt><dd className="cb-selectable">{value||"—"}</dd><Button size="small" disabled={!value} onClick={()=>void copyDetails(value)} aria-label={`${t("workflow.copy")} ${t(({Target:"col.target",Network:"workflow.network",Type:"workflow.type",Rule:"col.rule",Chain:"col.chains",Process:"col.process",ID:"workflow.id"} as Record<string,string>)[key])}`}>{t("workflow.copy")}</Button></div>)}</dl>
        <div className="actions"><span role="status">{copied?t("workflow.copied"):""}</span><Button onClick={()=>void copyDetails(JSON.stringify({...connectionDetails(selected),upload:selected.up,download:selected.down,elapsed_ms:selected.elapsed_ms},null,2))}>{t("workflow.copyAll")}</Button></div>
      </Dialog>}

    </div>
  );
}

function Empty({ text }: { text: string }) {
  return (
    <div
      className="flex items-center justify-center text-xs"
      style={{ minHeight: 160, color: "var(--cb-text-dim)" }}
    >
      {text}
    </div>
  );
}

function Row({ row, onClose, onInspect }: { row: ConnRow; onClose: () => void; onInspect:()=>void }) {
  const t = useT();
  return (
    <div
      className="cb-table-row cb-selectable cb-mono cb-connection-row"
      role="button" tabIndex={0} aria-label={`${t("conn.details")} ${row.host}`}
      onClick={e=>{if(!(e.target as Element).closest("button")&&!window.getSelection()?.toString())onInspect();}}
      onKeyDown={e=>{if(e.target===e.currentTarget&&(e.key==="Enter"||e.key===" ")){e.preventDefault();onInspect();}}}
      style={{
        height: ROW_H,
        gridTemplateColumns: CONNECTION_COLUMNS,
        borderBottom: "1px solid var(--cb-line)",
        fontSize: 13,
        color: "var(--cb-text-dim)",
      }}
    >
      <span
        className="truncate"
        title={row.host}
        style={{ color: "var(--cb-text)" }}
      >
        {row.network && (
          <span style={{ color: "var(--cb-faint)", marginRight: 6 }}>
            {row.network.toUpperCase()}
          </span>
        )}
        {row.host}
      </span>
      <span className="truncate" title={row.rule}>
        {row.rule}
      </span>
      <span className="truncate" title={nodeNameText(row.chains)}>
        <NodeName name={row.chains} />
      </span>
      <span className="flex items-center gap-1.5 truncate">
        <ProcessAvatar name={row.process} size={14} />
        {row.process}
      </span>
      <span className="text-right">{fmtBytes(row.up)}</span>
      <span className="text-right">{fmtBytes(row.down)}</span>
      <span className="text-right">{fmtDuration(row.elapsed_ms)}</span>
      <Button iconOnly size="small" tone="danger"
        onClick={onClose}
        aria-label={t("conn.close")}
        title={t("conn.close")}
      >
        ✕
      </Button>
    </div>
  );
}

// ---------- 按进程聚合 ----------

interface ProcessAgg {
  process: string;
  count: number;
  up: number;
  down: number;
  maxElapsed: number | null;
}

function aggregateByProcess(rows: ConnRow[]): ProcessAgg[] {
  const map = new Map<string, ProcessAgg>();
  for (const r of rows) {
    const key = r.process || " ";
    const cur = map.get(key) ?? {
      process: key,
      count: 0,
      up: 0,
      down: 0,
      maxElapsed: null,
    };
    cur.count += 1;
    cur.up += r.up;
    cur.down += r.down;
    if (r.elapsed_ms !== null) cur.maxElapsed = Math.max(cur.maxElapsed ?? 0, r.elapsed_ms);
    map.set(key, cur);
  }
  return Array.from(map.values()).sort(
    (a, b) => b.down + b.up - (a.down + a.up),
  );
}

function AppRow({
  agg,
  onInspect,
}: {
  agg: ProcessAgg;
  onInspect: () => void;
}) {
  const t = useT();
  return (
    <ButtonBase
      onClick={onInspect}
      title={t("conn.inspectHint")}
      className="cb-table-row w-full text-left"
      style={{
        height: 44,
        gridTemplateColumns: PROCESS_COLUMNS,
        borderBottom: "1px solid var(--cb-line)",
      }}
    >
      <span className="flex items-center gap-2 truncate">
        <ProcessAvatar name={agg.process} size={22} />
        <span
          className="cb-selectable truncate text-[12px]"
          style={{ color: "var(--cb-text)" }}
        >
          {agg.process.trim() ? agg.process : t("conn.unknownProcess")}
        </span>
      </span>
      <span
        className="cb-mono text-right text-[11.5px]"
        style={{ color: "var(--cb-text-dim)" }}
      >
        {agg.count}
      </span>
      <span className="cb-mono text-right text-[11.5px]">
        {fmtBytes(agg.up)}
      </span>
      <span className="cb-mono text-right text-[11.5px]">
        {fmtBytes(agg.down)}
      </span>
      <span
        className="cb-mono text-right text-[11.5px]"
        style={{ color: "var(--cb-faint)" }}
      >
        {fmtDuration(agg.maxElapsed)}
      </span>
    </ButtonBase>
  );
}

// ---------- 进程图标（首字母圆形 badge，色板哈希） ----------

const AVATAR_COLORS: [string, string][] = [
  ["rgba(122,162,255,.16)", "#7aa2ff"],
  ["rgba(157,140,255,.16)", "#9d8cff"],
  ["rgba(74,222,128,.14)", "#4ade80"],
  ["rgba(251,191,36,.14)", "#fbbf24"],
  ["rgba(248,113,113,.14)", "#f87171"],
  ["rgba(34,211,238,.14)", "#22d3ee"],
];

export function ProcessAvatar({
  name,
  size = 14,
}: {
  name: string;
  size?: number;
}) {
  const trimmed = name.trim();
  const letter = (trimmed[0] ?? "?").toUpperCase();
  let hash = 0;
  for (let i = 0; i < trimmed.length; i++) {
    hash = (hash * 31 + trimmed.charCodeAt(i)) >>> 0;
  }
  const [bg, fg] = AVATAR_COLORS[hash % AVATAR_COLORS.length];
  return (
    <span
      className="grid flex-none place-items-center rounded-full font-semibold"
      style={{
        width: size,
        height: size,
        background: bg,
        color: fg,
        fontSize: size * 0.58,
        lineHeight: 1,
      }}
    >
      {letter}
    </span>
  );
}
