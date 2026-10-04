/** 设置页：M0 含内核端口/局域网/外观/自启（S4），当前为骨架。 */
export default function SettingsPage() {
  return (
    <div className="mx-auto max-w-3xl space-y-5">
      <h1 className="text-lg font-semibold">设置</h1>
      <Group title="内核">
        <Row label="混合监听端口" hint="HTTP + SOCKS5 共用端口">
          <input disabled defaultValue="7897" className="w-24 rounded-lg border px-2 py-1 text-sm"
            style={{ borderColor: "var(--cb-border)", background: "var(--cb-surface)" }} />
        </Row>
        <Row label="允许局域网连接" hint="同一 Wi-Fi 下的其他设备可使用本机代理">
          <input disabled type="checkbox" />
        </Row>
      </Group>
      <Group title="外观">
        <Row label="主题" hint="跟随系统 / 浅色 / 深色">
          <select disabled className="rounded-lg border px-2 py-1 text-sm"
            style={{ borderColor: "var(--cb-border)", background: "var(--cb-surface)" }}>
            <option>跟随系统</option>
          </select>
        </Row>
        <Row label="语言">
          <select disabled className="rounded-lg border px-2 py-1 text-sm"
            style={{ borderColor: "var(--cb-border)", background: "var(--cb-surface)" }}>
            <option>简体中文</option>
            <option>English</option>
          </select>
        </Row>
      </Group>
      <Group title="备份">
        <Row label="配置快照" hint="每次保存前自动快照，保留最近 5 份">
          <span className="text-xs" style={{ color: "var(--cb-text-dim)" }}>自动</span>
        </Row>
      </Group>
    </div>
  );
}

function Group({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <section className="rounded-xl border"
      style={{ background: "var(--cb-surface)", borderColor: "var(--cb-border)" }}>
      <div className="border-b px-4 py-2.5 text-xs font-medium"
        style={{ borderColor: "var(--cb-border)", color: "var(--cb-text-dim)" }}>
        {title}
      </div>
      <div className="divide-y" style={{ borderColor: "var(--cb-border)" }}>
        {children}
      </div>
    </section>
  );
}

function Row({ label, hint, children }: { label: string; hint?: string; children: React.ReactNode }) {
  return (
    <div className="flex items-center justify-between px-4 py-3">
      <div>
        <div className="text-sm">{label}</div>
        {hint && (
          <div className="mt-0.5 text-xs" style={{ color: "var(--cb-text-dim)" }}>
            {hint}
          </div>
        )}
      </div>
      {children}
    </div>
  );
}
