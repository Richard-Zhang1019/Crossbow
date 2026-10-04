/** 代理页（M0-S3 接入内核 API）：策略组与节点测速。 */
export default function ProxiesPage() {
  return (
    <div className="mx-auto max-w-4xl space-y-5">
      <div className="flex items-center justify-between">
        <h1 className="text-lg font-semibold">代理</h1>
        <button
          disabled
          className="rounded-lg border px-3 py-1.5 text-xs opacity-50"
          style={{ borderColor: "var(--cb-border)" }}
        >
          ⚡ 测速全部
        </button>
      </div>
      <div
        className="flex h-64 flex-col items-center justify-center gap-2 rounded-xl border text-sm"
        style={{ background: "var(--cb-surface)", borderColor: "var(--cb-border)", color: "var(--cb-text-dim)" }}
      >
        <span>尚未接入内核</span>
        <span className="text-xs">先在「配置」页导入订阅并启动内核</span>
      </div>
    </div>
  );
}
