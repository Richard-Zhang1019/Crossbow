import ProfileSwitcher from "@/components/ProfileSwitcher";
import { UpdateEntry } from "@/components/UpdateEntry";
import { parseNavigation, type NavigationTarget } from "@/lib/workflow";
import { ButtonBase, Button } from "@/components/Button";
import { notify } from "@/notifications/service";
import { useEffect, useRef, useState } from "react";
import HomePage, { coreText } from "@/pages/Home";
import ProxiesPage from "@/pages/Proxies";
import ProfilesPage from "@/pages/Profiles";
import ConnectionsPage from "@/pages/Connections";
import DiagnosticsPage from "@/pages/Diagnostics";
import LogsPage from "@/pages/Logs";
import SettingsPage from "@/pages/Settings";
import { initLang, useT } from "@/i18n/i18n";
import { getCurrentWindow } from "@tauri-apps/api/window";
import logoGlyph from "./assets/logo-glyph.png";
import { Icon } from "@/components/Icon";

/** Overlay 标题栏拖拽：显式 startDragging（data-tauri-drag-region 在
 *  Overlay 样式下不可靠），主键按下才触发，不影响子元素交互。 */
export function dragWindow(e: React.MouseEvent) {
  if (e.button === 0) {
    e.preventDefault();
    getCurrentWindow().startDragging();
  }
}
import { ipc, type UiSettings, type PlatformCapabilities, type CoreStatus } from "@/services/ipc";
import {
  NotificationHistory,
  useNotificationNavigation,
} from "@/notifications/NotificationProvider";

const PAGES = [
  "home",
  "proxies",
  "connections",
  "logs",
  "diagnostics",
  "profiles",
  "settings",
] as const;
type PageId = (typeof PAGES)[number];
const isPage = (value: string): value is PageId => PAGES.includes(value as PageId);

/// 从版本串提取短版本：mihomo「… v1.19.32 …」→ v1.19.32；sing-box「sing-box version 1.14.2」→ 1.14.2
function shortVersion(version: string | undefined): string {
  const v = version ?? "";
  const vTok = v.match(/v\d+\.\d+\.\d+/);
  if (vTok) return vTok[0];
  const i = v.indexOf("version ");
  if (i >= 0)
    return (
      v
        .slice(i + 8)
        .trim()
        .split(/\s+/)[0] || v
    );
  return v.split(/\s+/).slice(0, 1).join(" ");
}

function applyTheme(theme: UiSettings["theme"]) {
  const dark =
    theme === "dark" ||
    (theme === "system" && window.matchMedia("(prefers-color-scheme: dark)").matches);
  document.documentElement.classList.toggle("dark", dark);
}

export default function App() {
  const t = useT();
  const isWindows = navigator.userAgent.includes("Windows");
  const isMacos = navigator.userAgent.includes("Macintosh");
  const [destination,setDestination]=useState<NavigationTarget|null>(null);
  const [page, setPage] = useState<PageId>("home");
  const [settingSection, setSettingSection] = useState<"network" | "appearance" | "logs" | "app">(
    "network",
  );
  const [configTab, setConfigTab] = useState<"sources" | "overrides">("sources");
  const [profileName, setProfileName] = useState("");
  const [appVersion, setAppVersion] = useState("");
  const [coreStatus, setCoreStatus] = useState<CoreStatus | null>(null);
  const [coreLabel, setCoreLabel] = useState<string>("");
  const [capabilities, setCapabilities] = useState<PlatformCapabilities | null>(null);
  const diagnosisRevision = useRef(0);
  const [diagnosisListeners, setDiagnosisListeners] = useState<Promise<void> | null>(null);
  const [, setDiagnosisRevision] = useState(0);

  useEffect(() => {
    let active = true;
    const invalidate = () => {
      if (!active) return;
      diagnosisRevision.current += 1;
      setDiagnosisRevision(diagnosisRevision.current);
    };
    const subscriptions = [
      ipc.onCoreStatus(invalidate),
      ipc.onSysproxyStatus(invalidate),
      ipc.onTunStatus(invalidate),
      ipc.onProfileSwitched(invalidate),
      ipc.onProxySelectionChanged(invalidate),
      ipc.onCoreMode(invalidate),
    ];
    const ready = Promise.all(subscriptions).then(() => {});
    void ready.catch(invalidate);
    setDiagnosisListeners(ready);
    return () => {
      active = false;
      for (const subscription of subscriptions) {
        void subscription.then(off => off()).catch(() => {});
      }
    };
  }, []);

  useEffect(() => {
    let active = true;
    ipc
      .platformCapabilities()
      .then((value) => {
        if (active) setCapabilities(value);
      })
      .catch((error) => notify.error(error, "platform-capabilities"));
    return () => {
      active = false;
    };
  }, []);

  useEffect(() => {
    let active = true;
    let receivedStatus = false;
    const un = ipc.onCoreStatus((status) => {
      receivedStatus = true;
      if (active) setCoreStatus(status);
    });
    void un
      .then(() => ipc.coreStatus())
      .then((status) => {
        if (active && !receivedStatus) setCoreStatus(status);
      })
      .catch(() => {});
    void ipc
      .appVersion()
      .then((value) => {
        if (active) setAppVersion(value);
      })
      .catch(() => {});
    return () => {
      active = false;
      void un.then((off) => off());
    };
  }, []);

  useEffect(() => {
    let alive = true, revision = 0;
    const refresh = async () => {
      const current = ++revision;
      try {
        const [rows,id] = await Promise.all([ipc.listProfiles(),ipc.activeProfileId()]);
        if(alive && current === revision) setProfileName(rows.find(p=>p.id===id)?.name ?? "");
      } catch { /* The switcher exposes its own read failure and retry. */ }
    };
    void refresh();
    // 后端事件：托盘菜单切换配置也走 set_active_profile，前端据此同步
    // 页头配置名并转发 DOM 事件（Home 等既有监听者一并刷新）。
    const unBackend = ipc.onProfileSwitched(() => {
      if (!alive) return;
      window.dispatchEvent(new CustomEvent("cb:profile-switched"));
      void refresh();
    });
    window.addEventListener("cb:profile-switched",refresh);
    return () => { alive=false; window.removeEventListener("cb:profile-switched",refresh); void unBackend.then((off)=>off()); };
  }, [page]);

  const refreshCoreLabel = () => {
    ipc
      .getEngineConfig()
      .then((e) =>
        ipc
          .coreBinaryInfoFor(e.engine)
          .then((info) =>
            setCoreLabel(
              `${e.engine === "singbox" ? "sing-box" : "mihomo"} ${shortVersion(info?.version)}`,
            ),
          ),
      )
      .catch((error) => notify.error(error, "app-settings-read"));
  };

  useEffect(() => {
    // 窗口底部的内核标识：跟随持久化引擎与其二进制版本；引擎切换时重取
    refreshCoreLabel();
    let unlisten: (() => void) | null = null;
    ipc.onEngineChanged(refreshCoreLabel).then((f) => {
      unlisten = f;
    });
    return () => {
      unlisten?.();
    };
  }, []);

  // 托盘「发现新版本」等入口跳转设置页
  useEffect(() => {
    const un = ipc.onGoto((p) => {
      if (isPage(p)) {
        setPage(p);
        if (p === "settings") setSettingSection("app");
        if (p === "profiles") setConfigTab("sources");
      }
    });
    return () => {
      void un.then((f) => f());
    };
  }, []);

  useEffect(() => {
    initLang();
    let currentTheme: UiSettings["theme"] = "system";
    const updateTheme = (theme: UiSettings["theme"]) => {
      currentTheme = theme;
      applyTheme(theme);
    };
    ipc
      .getUiSettings()
      .then((u) => updateTheme(u.theme))
      .catch((error) => notify.error(error, "app-settings-read"));
    const un = ipc.onTheme(updateTheme);
    const mq = window.matchMedia("(prefers-color-scheme: dark)");
    const onSystem = () => {
      if (currentTheme === "system") applyTheme("system");
    };
    mq.addEventListener("change", onSystem);
    return () => {
      void un.then((f) => f());
      mq.removeEventListener("change", onSystem);
    };
  }, []);

  useEffect(() => {
    const navigate = (event: Event) => {
      const next = (event as CustomEvent<string>).detail;
      if (isPage(next)) {
        setDestination(null);
        setPage(next);
        if (next === "settings") setSettingSection("app");
        if (next === "profiles") setConfigTab("sources");
      }
    };
    window.addEventListener("cb:goto", navigate);
    return () => window.removeEventListener("cb:goto", navigate);
  }, []);

  useNotificationNavigation(raw => {
    const target=parseNavigation(raw);if(!target||!isPage(target.page))return;
    setDestination(target);setPage(target.page);
    if(target.page==="settings")setSettingSection(target.settingSection??"app");
    if(target.page==="profiles")setConfigTab("sources");
  });
  useEffect(()=>{
    // Settings waits for its data and focuses the actual control, never the row.
    if(!destination || destination.page === "settings")return;
    const id=destination.control??(destination.checkId?`cb-check-${destination.checkId}`:undefined);
    if(!id)return;
    const frame=requestAnimationFrame(()=>{const el=document.getElementById(id)??(destination.page==="diagnostics"?document.querySelector<HTMLElement>(".cb-diagnostics-page button"):null);el?.scrollIntoView({block:"center"});el?.focus({preventScroll:true});});
    return()=>cancelAnimationFrame(frame);
  },[destination,page,settingSection]);

  const workspace = ["connections", "logs", "diagnostics"].includes(page) ? "monitor" : page;
  const go = (next: PageId) => {
    setDestination(null);
    setPage(next);
  };
  return (
    <div className={`cb-frame ${isWindows ? "is-windows" : isMacos ? "is-mac" : "is-linux"}`}>
      {isMacos && (
        <div
          className="cb-window-drag"
          data-tauri-drag-region
          onMouseDown={dragWindow}
          aria-hidden="true"
        />
      )}
      <header className="cb-global-nav">
        <ButtonBase className="cb-wordmark" onClick={() => go("home")} aria-label="Crossbow">
          <span
            className="cb-logo-glyph"
            aria-hidden="true"
            style={{
              maskImage: `url(${logoGlyph})`,
              WebkitMaskImage: `url(${logoGlyph})`,
            }}
          />
          <b>Crossbow</b>
        </ButtonBase>
        <nav className="cb-primary-nav" aria-label={t("studio.navigation")}>
          {(
            [
              ["home", "studio.connect"],
              ["proxies", "studio.nodes"],
              ["monitor", "studio.monitor"],
              ["profiles", "studio.config"],
              ["settings", "studio.settings"],
            ] as const
          ).map(([id, label]) => (
            <ButtonBase
              key={id}
              className={workspace === id ? "active" : ""}
              aria-current={workspace === id ? "page" : undefined}
              onClick={() => go(id === "monitor" ? "connections" : id)}
            >
              {t(label)}
            </ButtonBase>
          ))}
        </nav>
        <div className="cb-global-tools">
          {capabilities?.app_update && (
            <UpdateEntry onOpen={() => {
              setPage("settings");
              setSettingSection("app");
              setDestination({ page: "settings", settingSection: "app", control: "cb-update-status" });
            }} />
          )}
          <ProfileSwitcher currentName={profileName} onManage={()=>{setConfigTab("sources");go("profiles")}}/>
          <Button
            context="toolbar"
            className="cb-diagnostics-button"
            onClick={() => go("diagnostics")}
          >
            <Icon name="pulse" />
            {t("nav.diagnostics")}
          </Button>
          <NotificationHistory />
        </div>
      </header>
      <main className="cb-paper" id="main-content">
        {workspace === "monitor" && (
          <nav className="cb-subnav" aria-label={t("studio.monitor")}>
            {(
              [
                ["connections", "nav.connections"],
                ["logs", "nav.logs"],
                ["diagnostics", "nav.diagnostics"],
              ] as const
            ).map(([id, label]) => (
              <ButtonBase
                key={id}
                className={page === id ? "active" : ""}
                aria-current={page === id ? "page" : undefined}
                onClick={() => go(id)}
              >
                {t(label)}
              </ButtonBase>
            ))}
          </nav>
        )}
        {page === "profiles" && (
          <nav className="cb-subnav" aria-label={t("studio.config")}>
            <ButtonBase
              className={configTab === "sources" ? "active" : ""}
              onClick={() => {setDestination(null);setConfigTab("sources");}}
            >
              {t("studio.sources")}
            </ButtonBase>
            <ButtonBase
              className={configTab === "overrides" ? "active" : ""}
              onClick={() => {setDestination(null);setConfigTab("overrides");}}
            >
              {t("studio.overrides")}
            </ButtonBase>
          </nav>
        )}
        <div className={`cb-page-content cb-view-${page}`}>
          {page === "home" && <HomePage capabilities={capabilities} />}
          {page === "proxies" && <ProxiesPage />}
          {page === "profiles" && <ProfilesPage tab={configTab} focusProfileId={destination?.profileId} navigationRequest={destination} />}
          {page === "connections" && <ConnectionsPage />}
          {page === "diagnostics" && <DiagnosticsPage revision={diagnosisRevision} listeners={diagnosisListeners} appVersion={appVersion} />}
          {page === "logs" && <LogsPage />}
          {page === "settings" && (
            <div className="cb-context-layout cb-settings-layout">
              <nav className="cb-context-nav" aria-label={t("studio.settings")}>
                {(
                  [
                    ["network", "studio.network"],
                    ["appearance", "settings.appearance"],
                    ["logs", "settings.logsSection"],
                    ["app", "studio.application"],
                  ] as const
                ).map(([id, label]) => (
                  <ButtonBase
                    key={id}
                    className={settingSection === id ? "active" : ""}
                    aria-current={settingSection === id ? "page" : undefined}
                    onClick={() => {setDestination(null);setSettingSection(id);}}
                  >
                    {t(label)}
                  </ButtonBase>
                ))}
              </nav>
              <SettingsPage section={settingSection} capabilities={capabilities} focusTarget={destination} />
            </div>
          )}
        </div>
      </main>
      <footer className="cb-system-foot">
        <span>
          <i
            className={`cb-dot ${coreStatus?.state === "Running" ? "ok" : coreStatus?.state === "Crashed" ? "bad" : "idle"}`}
          />
          {coreStatus ? coreText(t, coreStatus) : t("home.statePending")} · {coreLabel || "—"}
        </span>
        <span>Crossbow {appVersion && `v${appVersion}`}</span>
      </footer>
    </div>
  );
}
