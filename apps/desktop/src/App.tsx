import { useEffect, useState } from "react";
import HomePage from "./pages/Home";
import ProxiesPage from "./pages/Proxies";
import ProfilesPage from "./pages/Profiles";
import ConnectionsPage from "./pages/Connections";
import DiagnosticsPage from "./pages/Diagnostics";
import LogsPage from "./pages/Logs";
import SettingsPage from "./pages/Settings";
import { initLang, useT } from "./i18n";
import { ipc, type UiSettings } from "./ipc";
import { Icon } from "./components/Icon";

const NAV = [
  {
    section: "proxy",
    items: [
      { id: "home", icon: "activity", key: "nav.home" },
      { id: "proxies", icon: "bolt", key: "nav.proxies" },
      { id: "connections", icon: "cube", key: "nav.connections" },
      { id: "logs", icon: "lines", key: "nav.logs" },
      { id: "diagnostics", icon: "pulse", key: "nav.diagnostics" },
    ],
  },
  {
    section: "general",
    items: [
      { id: "profiles", icon: "grid", key: "nav.profiles" },

      { id: "settings", icon: "gear", key: "nav.settings" },
    ],
  },
] as const;

type PageId = (typeof NAV)[number]["items"][number]["id"];

function applyTheme(theme: UiSettings["theme"]) {
  const dark =
    theme === "dark" ||
    (theme === "system" &&
      window.matchMedia("(prefers-color-scheme: dark)").matches);
  document.documentElement.classList.toggle("dark", dark);
}

export default function App() {
  const t = useT();
  const [page, setPage] = useState<PageId>("home");
  const [toast, setToast] = useState<string | null>(null);

  useEffect(() => {
    initLang();
    ipc.getUiSettings().then((u) => applyTheme(u.theme)).catch(() => {});
    const un = ipc.onTheme(applyTheme);
    const mq = window.matchMedia("(prefers-color-scheme: dark)");
    const onSystem = () => applyTheme("system");
    mq.addEventListener("change", onSystem);
    return () => {
      void un.then((f) => f());
      mq.removeEventListener("change", onSystem);
    };
  }, []);

  useEffect(() => {
    const un1 = ipc.onProfileImported((url) =>
      setToast(t("toast.imported", { url: url.slice(0, 60) })),
    );
    const un2 = ipc.onProfileImportFailed((msg) => setToast(t("toast.importFailed", { msg })));
    return () => {
      for (const un of [un1, un2]) void un.then((f) => f());
    };
  }, [t]);

  return (
    <div className="flex h-full">
      <aside
        className="flex w-52 shrink-0 flex-col gap-px border-r py-5 px-3"
        style={{ borderColor: "var(--cb-line)" }}
      >
        <div className="mb-5 flex items-center gap-2.5 px-2.5">
          <div
            className="grid h-6 w-6 place-items-center rounded-md"
            style={{ background: "linear-gradient(135deg, #7aa2ff, #9d8cff)" }}
          >
            <Icon name="crossbow" size={12} stroke="#0a0c10" />
          </div>
          <b className="text-[13.5px] font-semibold tracking-tight">Crossbow</b>
        </div>
        {NAV.map((group) => (
          <div key={group.section}>
            <div className="cb-micro px-2.5 pb-1.5 pt-3.5">{t(group.section === "proxy" ? "section.proxy" : "section.general")}</div>
            {group.items.map((item) => {
              const on = page === item.id;
              return (
                <button
                  key={item.id}
                  onClick={() => setPage(item.id)}
                  className="flex w-full items-center gap-2.5 rounded-lg px-2.5 py-[7px] text-left text-[12.5px]"
                  style={{
                    color: on ? "var(--cb-text)" : "var(--cb-text-dim)",
                    background: on ? "var(--cb-surface-2)" : "transparent",
                    boxShadow: on ? "inset 0 0 0 1px var(--cb-line)" : "none",
                  }}
                >
                  <Icon
                    name={item.icon}
                    size={15}
                    stroke={on ? "var(--cb-accent)" : "var(--cb-faint)"}
                  />
                  {t(item.key)}
                </button>
              );
            })}
          </div>
        ))}
        <div
          className="mt-auto flex items-center gap-2 border-t px-2.5 pt-3 text-[10.5px]"
          style={{ borderColor: "var(--cb-line)", color: "var(--cb-faint)" }}
        >
          <span className="cb-dot idle" />
          mihomo <span className="cb-mono">v1.19.32</span>
        </div>
      </aside>

      <main
        className="relative flex-1 overflow-y-auto px-7 py-6"
        style={{
          background:
            "radial-gradient(900px 420px at 75% -8%, color-mix(in srgb, var(--cb-accent) 5%, transparent), transparent 60%)",
        }}
      >
        {page === "home" && <HomePage />}
        {page === "proxies" && <ProxiesPage />}
        {page === "profiles" && <ProfilesPage />}
        {page === "connections" && <ConnectionsPage />}
        {page === "diagnostics" && <DiagnosticsPage />}
        {page === "logs" && <LogsPage />}
        {page === "settings" && <SettingsPage />}

        {toast && (
          <div
            className="cb-selectable absolute bottom-5 left-1/2 -translate-x-1/2 rounded-lg border px-4 py-2 text-xs"
            style={{
              background: "var(--cb-surface-2)",
              borderColor: "var(--cb-line-strong)",
              color: "var(--cb-text)",
              boxShadow: "0 8px 24px rgba(0,0,0,.35)",
            }}
          >
            {toast}
            <button className="ml-3 opacity-60 hover:opacity-100" onClick={() => setToast(null)}>
              ✕
            </button>
          </div>
        )}
      </main>
    </div>
  );
}
