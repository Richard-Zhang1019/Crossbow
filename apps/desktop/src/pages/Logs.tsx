/** 日志页（M0-S4）：内核日志实时滚动 + 级别过滤，虚拟滚动。 */
export default function LogsPage() {
  return (
    <div className="mx-auto max-w-5xl space-y-5">
      <div className="flex items-center gap-3">
        <h1 className="text-lg font-semibold">日志</h1>
        {["debug", "info", "warning", "error"].map((lv) => (
          <label key={lv} className="flex items-center gap-1 text-xs"
            style={{ color: "var(--cb-text-dim)" }}>
            <input type="checkbox" defaultChecked={lv !== "debug"} />
            {lv}
          </label>
        ))}
      </div>
      <div
        className="cb-selectable flex h-72 items-center justify-center rounded-xl border font-mono text-xs"
        style={{ background: "var(--cb-surface)", borderColor: "var(--cb-border)", color: "var(--cb-text-dim)" }}
      >
        接入内核 logs WS 后启用
      </div>
    </div>
  );
}
