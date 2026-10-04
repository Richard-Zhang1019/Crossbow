import { useState } from "react";
import HomePage from "./pages/Home";
import ProxiesPage from "./pages/Proxies";
import ProfilesPage from "./pages/Profiles";
import ConnectionsPage from "./pages/Connections";
import LogsPage from "./pages/Logs";
import SettingsPage from "./pages/Settings";

const NAV = [
  { id: "home", label: "首页", icon: "◆" },
  { id: "proxies", label: "代理", icon: "⚡" },
  { id: "profiles", label: "配置", icon: "▤" },
  { id: "connections", label: "连接", icon: "⇄" },
  { id: "logs", label: "日志", icon: "☰" },
  { id: "settings", label: "设置", icon: "⚙" },
] as const;

type PageId = (typeof NAV)[number]["id"];

export default function App() {
  const [page, setPage] = useState<PageId>("home");

  return (
    <div className="flex h-full">
      <aside className="flex w-44 shrink-0 flex-col gap-0.5 border-r p-3"
        style={{ background: "var(--cb-surface)", borderColor: "var(--cb-border)" }}>
        <div className="mb-4 flex items-center gap-2 px-2 pt-1">
          <span className="text-lg" style={{ color: "var(--cb-accent)" }}>⌖</span>
          <span className="text-sm font-semibold tracking-wide">Crossbow</span>
        </div>
        {NAV.map((item) => (
          <button
            key={item.id}
            onClick={() => setPage(item.id)}
            className="flex items-center gap-2.5 rounded-lg px-3 py-2 text-left text-[13px] transition-colors"
            style={
              page === item.id
                ? { background: "var(--cb-accent)", color: "#fff" }
                : { color: "var(--cb-text-dim)" }
            }
          >
            <span className="w-4 text-center">{item.icon}</span>
            {item.label}
          </button>
        ))}
        <div className="mt-auto px-3 text-[11px]" style={{ color: "var(--cb-text-dim)" }}>
          v0.1.0 · mihomo
        </div>
      </aside>

      <main className="flex-1 overflow-y-auto p-6">
        {page === "home" && <HomePage />}
        {page === "proxies" && <ProxiesPage />}
        {page === "profiles" && <ProfilesPage />}
        {page === "connections" && <ConnectionsPage />}
        {page === "logs" && <LogsPage />}
        {page === "settings" && <SettingsPage />}
      </main>
    </div>
  );
}
