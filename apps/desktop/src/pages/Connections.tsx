import { useEffect, useMemo, useRef, useState } from "react";
import VirtualList from "../components/VirtualList";
import { fmtBytes, fmtDuration } from "../format";
import { ipc, type ConnRow, type ConnSnapshot } from "../ipc";
import { useT } from "../i18n";
import { PageHead } from "./Home";

const ROW_H = 30;
const BODY_H = 480;

/** 连接页：列表 / 按进程聚合双视图（内核 connections WS → Rust 节流聚合）。 */
export default function ConnectionsPage() {
  const t = useT();
  const [snapshot, setSnapshot] = useState<ConnSnapshot>({
    rows: [],
    upload_total: 0,
    download_total: 0,
    memory: 0,
  });
  const [filter, setFilter] = useState("");
  const [paused, setPaused] = useState(false);
  const [view, setView] = useState<"list" | "apps">("list");
  const latest = useRef<ConnSnapshot>(snapshot);

  useEffect(() => {
    ipc.subscribeConnections().catch(console.error);
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
      console.error(e);
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
    if (!q) return snapshot.rows;
    return snapshot.rows.filter((r) =>
      [r.host, r.rule, r.chains, r.process].some((f) => f.toLowerCase().includes(q)),
    );
  }, [snapshot, filter]);

  const apps = useMemo(() => aggregateByProcess(rows), [rows]);

  return (
    <div className="mx-auto max-w-6xl space-y-3">
      <PageHead
        title={t("conn.title")}
        meta={`${snapshot.truncated ? `${snapshot.truncated} 条（显示前 ${rows.length}）` : `${rows.length} 条`} · ↑${fmtBytes(snapshot.upload_total)} ↓${fmtBytes(snapshot.download_total)} · ${t("conn.mem")} ${fmtBytes(snapshot.memory)}`}
      >
        <div className="cb-seg">
          <button className={view === "list" ? "on" : ""} onClick={() => setView("list")}>
            {t("conn.viewList")}
          </button>
          <button className={view === "apps" ? "on" : ""} onClick={() => setView("apps")}>
            {t("conn.viewApps")}
          </button>
        </div>
        <input
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
          placeholder={t("conn.filter")}
          className="cb-input cb-selectable w-64"
        />
        <button className="cb-btn" onClick={togglePause}>
          {paused ? t("conn.paused") : t("conn.pause")}
        </button>
      </PageHead>

      <div className="cb-card grow overflow-hidden" style={{ padding: 0 }}>
        {view === "list" ? (
          <>
            <div
              className="cb-th grid px-3 py-2.5"
              style={{
                gridTemplateColumns:
                  "minmax(220px,2fr) minmax(120px,1fr) minmax(150px,1.2fr) 110px 90px 90px 70px 24px",
                borderBottom: "1px solid var(--cb-line)",
              }}
            >
              <span>{t("col.target")}</span>
              <span>{t("col.rule")}</span>
              <span>{t("col.chains")}</span>
              <span>{t("col.process")}</span>
              <span className="text-right">{t("col.up")}</span>
              <span className="text-right">{t("col.down")}</span>
              <span className="text-right">{t("col.elapsed")}</span>
              <span style={{ width: 24 }} />
            </div>
            {rows.length === 0 ? (
              <Empty text={snapshot.rows.length === 0 ? t("conn.empty") : t("conn.noMatch")} />
            ) : (
              <VirtualList
                items={rows}
                rowHeight={ROW_H}
                height={BODY_H}
                render={(r: ConnRow) => (
                  <Row key={r.id} row={r} onClose={() => closeOne(r.id)} />
                )}
              />
            )}
          </>
        ) : (
          <>
            <div
              className="cb-th grid px-3 py-2.5"
              style={{
                gridTemplateColumns: "minmax(200px,1.6fr) 90px 100px 100px 90px",
                borderBottom: "1px solid var(--cb-line)",
              }}
            >
              <span>{t("col.process")}</span>
              <span className="text-right">{t("conn.connCount")}</span>
              <span className="text-right">{t("col.up")}</span>
              <span className="text-right">{t("col.down")}</span>
              <span className="text-right">{t("col.elapsed")}</span>
            </div>
            {apps.length === 0 ? (
              <Empty text={snapshot.rows.length === 0 ? t("conn.empty") : t("conn.noMatch")} />
            ) : (
              <VirtualList
                items={apps}
                rowHeight={44}
                height={BODY_H}
                render={(a: ProcessAgg) => (
                  <AppRow
                    key={a.process}
                    agg={a}
                    onInspect={() => {
                      setFilter(a.process);
                      setView("list");
                    }}
                  />
                )}
              />
            )}
          </>
        )}
      </div>
    </div>
  );
}

function Empty({ text }: { text: string }) {
  return (
    <div
      className="flex items-center justify-center text-xs"
      style={{ height: BODY_H, color: "var(--cb-text-dim)" }}
    >
      {text}
    </div>
  );
}

function Row({ row, onClose }: { row: ConnRow; onClose: () => void }) {
  const t = useT();
  return (
    <div
      className="cb-selectable cb-mono grid items-center px-3"
      style={{
        height: ROW_H,
        gridTemplateColumns:
          "minmax(220px,2fr) minmax(120px,1fr) minmax(150px,1.2fr) 110px 90px 90px 70px 24px",
        borderBottom: "1px solid var(--cb-line)",
        fontSize: 11.5,
        color: "var(--cb-text-dim)",
      }}
    >
      <span className="truncate" title={row.host} style={{ color: "var(--cb-text)" }}>
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
      <span className="truncate" title={row.chains}>
        {row.chains}
      </span>
      <span className="flex items-center gap-1.5 truncate">
        <ProcessAvatar name={row.process} size={14} />
        {row.process}
      </span>
      <span className="text-right">{fmtBytes(row.up)}</span>
      <span className="text-right">{fmtBytes(row.down)}</span>
      <span className="text-right">{fmtDuration(row.elapsed_ms)}</span>
      <button
        onClick={onClose}
        title={t("conn.close")}
        className="text-[13px] leading-none hover:opacity-80"
        style={{ color: "var(--cb-bad)" }}
      >
        ✕
      </button>
    </div>
  );
}

// ---------- 按进程聚合 ----------

interface ProcessAgg {
  process: string;
  count: number;
  up: number;
  down: number;
  maxElapsed: number;
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
      maxElapsed: 0,
    };
    cur.count += 1;
    cur.up += r.up;
    cur.down += r.down;
    cur.maxElapsed = Math.max(cur.maxElapsed, r.elapsed_ms);
    map.set(key, cur);
  }
  return Array.from(map.values()).sort(
    (a, b) => b.down + b.up - (a.down + a.up),
  );
}

function AppRow({ agg, onInspect }: { agg: ProcessAgg; onInspect: () => void }) {
  const t = useT();
  return (
    <button
      onClick={onInspect}
      title={t("conn.inspectHint")}
      className="grid w-full items-center px-3 text-left"
      style={{
        height: 44,
        gridTemplateColumns: "minmax(200px,1.6fr) 90px 100px 100px 90px",
        borderBottom: "1px solid var(--cb-line)",
      }}
    >
      <span className="flex items-center gap-2 truncate">
        <ProcessAvatar name={agg.process} size={22} />
        <span className="cb-selectable truncate text-[12px]" style={{ color: "var(--cb-text)" }}>
          {agg.process.trim() ? agg.process : t("conn.unknownProcess")}
        </span>
      </span>
      <span className="cb-mono text-right text-[11.5px]" style={{ color: "var(--cb-text-dim)" }}>
        {agg.count}
      </span>
      <span className="cb-mono text-right text-[11.5px]">{fmtBytes(agg.up)}</span>
      <span className="cb-mono text-right text-[11.5px]">{fmtBytes(agg.down)}</span>
      <span className="cb-mono text-right text-[11.5px]" style={{ color: "var(--cb-faint)" }}>
        {fmtDuration(agg.maxElapsed)}
      </span>
    </button>
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

export function ProcessAvatar({ name, size = 14 }: { name: string; size?: number }) {
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
