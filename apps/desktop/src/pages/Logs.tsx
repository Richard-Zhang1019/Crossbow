import { useEffect, useMemo, useRef, useState } from "react";
import { ipc, type LogEntry } from "../ipc";

const MAX_LINES = 2000;
const RENDER_TAIL = 500;

const LEVELS = ["debug", "info", "warning", "error"] as const;
type Level = (typeof LEVELS)[number];

const LEVEL_COLOR: Record<string, string> = {
  debug: "#98989d",
  info: "var(--cb-text)",
  warning: "#d6a02b",
  error: "#e0524c",
};

/** 日志页：内核日志实时滚动 + 级别过滤；缓冲上限 2000 行。 */
export default function LogsPage() {
  const [lines, setLines] = useState<LogEntry[]>([]);
  const [enabled, setEnabled] = useState<Record<Level, boolean>>({
    debug: false,
    info: true,
    warning: true,
    error: true,
  });
  const [keyword, setKeyword] = useState("");
  const pinned = useRef(true);
  const boxRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    ipc.subscribeLogs().catch(console.error);
    const un = ipc.onLogs((entries) => {
      setLines((prev) => {
        const next = [...prev, ...entries];
        return next.length > MAX_LINES ? next.slice(next.length - MAX_LINES) : next;
      });
    });
    return () => {
      void un.then((f) => f());
      ipc.unsubscribeLogs().catch(() => {});
    };
  }, []);

  // 自动滚动：用户上滚超过 40px 即暂停跟随
  useEffect(() => {
    if (pinned.current && boxRef.current) {
      boxRef.current.scrollTop = boxRef.current.scrollHeight;
    }
  }, [lines]);

  const shown = useMemo(() => {
    const kw = keyword.trim().toLowerCase();
    return lines
      .filter((l) => enabled[(l.type as Level) ?? "info"] ?? true)
      .filter((l) => !kw || l.payload.toLowerCase().includes(kw))
      .slice(-RENDER_TAIL);
  }, [lines, enabled, keyword]);

  return (
    <div className="mx-auto max-w-6xl space-y-3">
      <div className="flex items-center gap-3">
        <h1 className="text-lg font-semibold">日志</h1>
        {LEVELS.map((lv) => (
          <label key={lv} className="flex items-center gap-1 text-xs"
            style={{ color: "var(--cb-text-dim)" }}>
            <input
              type="checkbox"
              checked={enabled[lv]}
              onChange={(e) => setEnabled({ ...enabled, [lv]: e.target.checked })}
            />
            {lv}
          </label>
        ))}
        <div className="flex-1" />
        <input
          value={keyword}
          onChange={(e) => setKeyword(e.target.value)}
          placeholder="关键字过滤"
          className="cb-selectable w-56 rounded-lg border px-3 py-1.5 text-xs outline-none focus:border-[var(--cb-accent)]"
          style={{ borderColor: "var(--cb-border)", background: "var(--cb-surface)" }}
        />
        <button
          onClick={() => setLines([])}
          className="rounded-lg border px-3 py-1.5 text-xs"
          style={{ borderColor: "var(--cb-border)", color: "var(--cb-text-dim)" }}
        >
          清空
        </button>
      </div>

      <div
        ref={boxRef}
        onScroll={(e) => {
          const el = e.currentTarget;
          pinned.current = el.scrollHeight - el.scrollTop - el.clientHeight < 40;
        }}
        className="cb-selectable rounded-xl border font-mono text-[11px] leading-5"
        style={{
          height: 480,
          overflowY: "auto",
          background: "var(--cb-surface)",
          borderColor: "var(--cb-border)",
        }}
      >
        {shown.length === 0 ? (
          <div className="flex h-full items-center justify-center"
            style={{ color: "var(--cb-text-dim)" }}>
            内核未运行或暂无日志
          </div>
        ) : (
          shown.map((l, i) => (
            <div key={i} className="px-3" style={{ color: LEVEL_COLOR[l.type] ?? "var(--cb-text)" }}>
              {l.payload}
            </div>
          ))
        )}
      </div>
    </div>
  );
}
