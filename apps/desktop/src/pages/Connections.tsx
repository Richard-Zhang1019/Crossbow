import { useEffect, useMemo, useRef, useState } from "react";
import VirtualList from "../components/VirtualList";
import { fmtBytes, fmtDuration } from "../format";
import { ipc, type ConnRow, type ConnSnapshot } from "../ipc";
import { useT } from "../i18n";

const ROW_H = 30;
const BODY_H = 480;

/** 连接页：实时连接表（内核 connections WS → Rust 节流聚合 → 此处渲染）。 */
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

  return (
    <div className="mx-auto max-w-6xl space-y-3">
      <div className="flex items-center gap-3">
        <h1 className="text-lg font-semibold">{t("conn.title")}</h1>
        <span className="text-xs" style={{ color: "var(--cb-text-dim)" }}>
          {snapshot.truncated
            ? `${snapshot.truncated} 条（显示前 ${rows.length}）`
            : `${rows.length} 条`}{" "}
          · ↑{fmtBytes(snapshot.upload_total)} ↓
          {fmtBytes(snapshot.download_total)} · {t("conn.mem")} {fmtBytes(snapshot.memory)}
        </span>
        <div className="flex-1" />
        <input
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
          placeholder={t("conn.filter")}
          className="cb-selectable w-72 rounded-lg border px-3 py-1.5 text-xs outline-none focus:border-[var(--cb-accent)]"
          style={{ borderColor: "var(--cb-border)", background: "var(--cb-surface)" }}
        />
        <button
          onClick={togglePause}
          className="rounded-lg border px-3 py-1.5 text-xs"
          style={{
            borderColor: "var(--cb-border)",
            background: paused ? "var(--cb-accent)" : "transparent",
            color: paused ? "#fff" : "var(--cb-text-dim)",
          }}
        >
          {paused ? t("conn.paused") : t("conn.pause")}
        </button>
      </div>

      <div className="cb-card grow overflow-hidden text-xs" style={{ padding: 0 }}>
        <div
          className="grid px-3 py-2.5"
          style={{
            gridTemplateColumns: "minmax(220px,2fr) minmax(120px,1fr) minmax(150px,1.2fr) 90px 90px 90px 70px",
            color: "var(--cb-faint)",
            borderBottom: "1px solid var(--cb-line)",
            fontSize: 10,
            letterSpacing: ".1em",
            textTransform: "uppercase",
          }}
        >
          <span>{t("col.target")}</span>
          <span>{t("col.rule")}</span>
          <span>{t("col.chains")}</span>
          <span>{t("col.process")}</span>
          <span className="text-right">{t("col.up")}</span>
          <span className="text-right">{t("col.down")}</span>
          <span className="text-right">{t("col.elapsed")}</span>
        </div>
        {rows.length === 0 ? (
          <div
            className="flex items-center justify-center text-xs"
            style={{ height: BODY_H, color: "var(--cb-text-dim)" }}
          >
            {snapshot.rows.length === 0 ? t("conn.empty") : t("conn.noMatch")}
          </div>
        ) : (
          <VirtualList
            items={rows}
            rowHeight={ROW_H}
            height={BODY_H}
            render={(r: ConnRow) => <Row key={r.id} row={r} />}
          />
        )}
      </div>
    </div>
  );
}

function Row({ row }: { row: ConnRow }) {
  return (
    <div
      className="cb-selectable cb-mono grid items-center px-3"
      style={{
        height: ROW_H,
        gridTemplateColumns: "minmax(220px,2fr) minmax(120px,1fr) minmax(150px,1.2fr) 90px 90px 90px 70px",
        borderBottom: "1px solid rgba(255,255,255,.035)",
        fontSize: 11.5,
        color: "var(--cb-text-dim)",
      }}
    >
      <span className="truncate" title={row.host} style={{ color: "var(--cb-text)" }}>
        {row.network && <span style={{ color: "var(--cb-faint)", marginRight: 6 }}>{row.network.toUpperCase()}</span>}
        {row.host}
      </span>
      <span className="truncate" title={row.rule}>{row.rule}</span>
      <span className="truncate" title={row.chains}>{row.chains}</span>
      <span className="truncate">{row.process}</span>
      <span className="text-right">{fmtBytes(row.up)}</span>
      <span className="text-right">{fmtBytes(row.down)}</span>
      <span className="text-right">
        {fmtDuration(row.elapsed_ms)}
      </span>
    </div>
  );
}
