import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

/** 与 Rust 侧对齐的类型。 */
export interface Profile {
  id: string;
  name: string;
  kind: "remote" | "local";
  url?: string | null;
  update_interval_min: number;
  override_ids: string[];
  last_updated?: number | null;
  traffic?: TrafficInfo | null;
}

export interface TrafficInfo {
  upload: number;
  download: number;
  total: number;
  expire: number;
}

export type CoreState = "Stopped" | "Starting" | "Running" | "Crashed";

export interface CoreStatus {
  state: CoreState;
  message?: string;
}

export interface SysProxyStatus {
  enabled: boolean;
  port: number;
}

export interface UiSettings {
  theme: "system" | "light" | "dark";
  lang: string;
}

export interface EngineConfigView {
  engine: string;
  mixed_port: number;
  allow_lan: boolean;
}

export interface ConnRow {
  id: string;
  host: string;
  rule: string;
  chains: string;
  up: number;
  down: number;
  network: string;
  kind: string;
  process: string;
  elapsed_ms: number;
}

export interface ConnSnapshot {
  rows: ConnRow[];
  upload_total: number;
  download_total: number;
  memory: number;
}

export interface LogEntry {
  type: string;
  payload: string;
}

export const CORE_MODES = [
  { id: "direct", label: "直连" },
  { id: "rule", label: "规则" },
  { id: "global", label: "全局" },
] as const;

export type CoreMode = (typeof CORE_MODES)[number]["id"];

export const ipc = {
  listProfiles: () => invoke<Profile[]>("list_profiles"),
  activeProfileId: () => invoke<string | null>("active_profile_id"),
  importProfileUrl: (url: string, name?: string) =>
    invoke<Profile>("import_profile_url", { url, name: name ?? null }),
  importProfileContent: (name: string, content: string) =>
    invoke<Profile>("import_profile_content", { name, content }),
  updateProfile: (id: string) => invoke<Profile>("update_profile", { id }),
  setActiveProfile: (id: string) => invoke<void>("set_active_profile", { id }),
  removeProfile: (id: string) => invoke<void>("remove_profile", { id }),
  coreStart: () => invoke<void>("core_start"),
  coreStop: () => invoke<void>("core_stop"),
  coreStatus: () => invoke<CoreStatus>("core_status"),
  sysproxyStatus: () => invoke<SysProxyStatus>("sysproxy_status"),
  sysproxyToggle: () => invoke<void>("sysproxy_toggle"),
  coreMode: () => invoke<CoreMode>("core_mode"),
  setCoreMode: (mode: CoreMode) => invoke<void>("set_core_mode", { mode }),
  getUiSettings: () => invoke<UiSettings>("get_ui_settings"),
  getEngineConfig: () => invoke<EngineConfigView>("get_engine_config"),
  setTheme: (theme: UiSettings["theme"]) => invoke<void>("set_theme", { theme }),
  setMixedPort: (port: number) => invoke<void>("set_mixed_port", { port }),
  setAllowLan: (enabled: boolean) => invoke<void>("set_allow_lan", { enabled }),
  autostartStatus: () => invoke<boolean>("autostart_status"),
  autostartSet: (enable: boolean) => invoke<void>("autostart_set", { enable }),
  coreVersion: () => invoke<string>("core_version"),
  subscribeTraffic: () => invoke<void>("subscribe_traffic"),
  unsubscribeTraffic: () => invoke<void>("unsubscribe_traffic"),
  subscribeConnections: () => invoke<void>("subscribe_connections"),
  unsubscribeConnections: () => invoke<void>("unsubscribe_connections"),
  subscribeLogs: () => invoke<void>("subscribe_logs"),
  unsubscribeLogs: () => invoke<void>("unsubscribe_logs"),
  onCoreStatus: (cb: (s: CoreStatus) => void) =>
    listen<CoreStatus>("core://status", (e) => cb(e.payload)),
  onSysproxyStatus: (cb: (s: SysProxyStatus) => void) =>
    listen<SysProxyStatus>("sysproxy://status", (e) => cb(e.payload)),
  onCoreMode: (cb: (m: CoreMode) => void) =>
    listen<CoreMode>("core://mode", (e) => cb(e.payload)),
  onTraffic: (cb: (t: { up: number; down: number }) => void) =>
    listen<{ up: number; down: number }>("traffic://data", (e) => cb(e.payload)),
  onConnections: (cb: (s: ConnSnapshot) => void) =>
    listen<ConnSnapshot>("connections://data", (e) => cb(e.payload)),
  onLogs: (cb: (entries: LogEntry[]) => void) =>
    listen<LogEntry[]>("logs://data", (e) => cb(e.payload)),
  onTheme: (cb: (t: UiSettings["theme"]) => void) =>
    listen<UiSettings["theme"]>("ui://theme", (e) => cb(e.payload)),
  onProfileImported: (cb: (url: string) => void) =>
    listen<string>("profile://imported", (e) => cb(e.payload)),
  onProfileImportFailed: (cb: (msg: string) => void) =>
    listen<string>("profile://import-failed", (e) => cb(e.payload)),
};
