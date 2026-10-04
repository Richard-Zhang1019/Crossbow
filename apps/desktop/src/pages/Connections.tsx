/** 连接页（M0-S4）：实时连接表，2fps 节流。 */
export default function ConnectionsPage() {
  return (
    <div className="mx-auto max-w-5xl space-y-5">
      <div className="flex items-center justify-between">
        <h1 className="text-lg font-semibold">连接</h1>
        <input
          disabled
          placeholder="过滤：域名 / 进程 / 规则"
          className="w-64 rounded-lg border px-3 py-1.5 text-xs outline-none opacity-50"
          style={{ borderColor: "var(--cb-border)", background: "var(--cb-surface)" }}
        />
      </div>
      <div
        className="flex h-64 items-center justify-center rounded-xl border text-xs"
        style={{ background: "var(--cb-surface)", borderColor: "var(--cb-border)", color: "var(--cb-text-dim)" }}
      >
        目标 · 规则 · 链路 · 上下行 · 耗时（接入内核 connections WS 后启用）
      </div>
    </div>
  );
}
