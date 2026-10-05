import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

/** 与 Rust 侧对齐的类型。 */
export interface UpdateOutcome {
  ok: boolean;
  detail?: string | null;
  at: number;
}

export interface Profile {
  id: string;
  name: string;
  kind: "remote" | "local";
  url?: string | null;
  update_interval_min: number;
  override_ids: string[];
  last_updated?: number | null;
  traffic?: TrafficInfo | null;
  last_update?: UpdateOutcome | null;
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
  lightweight_close: boolean;
}

export interface EngineConfigView {
  engine: string;
  mixed_port: number;
  allow_lan: boolean;
  flag_emoji: boolean;
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
  truncated?: number;
}

export interface LogEntry {
  type: string;
  payload: string;
}

export interface CheckResult {
  id: string;
  ok: boolean;
  detail: string;
  fix: string | null;
}

export interface OutboundInfo {
  mode: string;
  node: string;
  delay_ms: number | null;
  chain: string[];
}

export interface NodeView {
  name: string;
  kind: string;
  alive: boolean;
  delay_ms: number | null;
}

export interface GroupView {
  name: string;
  kind: string;
  now: string | null;
  nodes: NodeView[];
}

export const CORE_MODES = [
  { id: "direct", label: "直连" },
  { id: "rule", label: "规则" },
  { id: "global", label: "全局" },
] as const;

export type CoreMode = (typeof CORE_MODES)[number]["id"];

export interface CoreBinaryInfo {
  path: string;
  source: "builtin" | "data" | "missing";
  version: string;
}

export interface InstallProgress {
  stage: string;
  detail: string;
}

/** 防御性解析：Tauri 事件载荷理论上是对象，若收到字符串则手动解析。 */
function parsePayload<T>(raw: unknown): T {
  if (typeof raw === "string") {
    try {
      return JSON.parse(raw) as T;
    } catch {
      return raw as T;
    }
  }
  return raw as T;
}

export const ipc = {
  checkUpdate: async (): Promise<string | null> => {
    const { check } = await import("@tauri-apps/plugin-updater");
    const update = await check();
    return update?.version ?? null;
  },
  applyUpdate: async () => {
    const { check } = await import("@tauri-apps/plugin-updater");
    const update = await check();
    if (update) {
      await update.downloadAndInstall();
      await import("@tauri-apps/plugin-process").then((m) => m.relaunch()).catch(() => {});
    }
  },

  coreBinaryInfo: () => invoke<CoreBinaryInfo>("core_binary_info"),
  coreInstall: () => invoke<CoreBinaryInfo>("core_install"),
  onInstallProgress: (cb: (p: InstallProgress) => void) =>
    listen<InstallProgress>("core://install-progress", (e) => cb(e.payload)),

  listProfiles: () => invoke<Profile[]>("list_profiles"),
  activeProfileId: () => invoke<string | null>("active_profile_id"),
  importProfileUrl: (url: string, name?: string) =>
    invoke<Profile>("import_profile_url", { url, name: name ?? null }),
  importProfileContent: (name: string, content: string) =>
    invoke<Profile>("import_profile_content", { name, content }),
  importProfileFile: (path: string) =>
    invoke<Profile>("import_profile_file", { path }),
  updateProfile: (id: string) => invoke<Profile>("update_profile", { id }),
  setProfileInterval: (id: string, intervalMin: number) =>
    invoke<void>("set_profile_interval", { id, intervalMin }),
  renameProfile: (id: string, name: string) =>
    invoke<void>("rename_profile", { id, name }),
  setFlagEmoji: (enabled: boolean) => invoke<void>("set_flag_emoji", { enabled }),
  onProfileUpdated: (cb: (id: string) => void) =>
    listen<string>("profile://updated", (e) => cb(e.payload)),
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
  setLang: (lang: string) => invoke<void>("set_lang", { lang }),
  setLightweightClose: (enabled: boolean) =>
    invoke<void>("set_lightweight_close", { enabled }),
  setMixedPort: (port: number) => invoke<void>("set_mixed_port", { port }),
  setAllowLan: (enabled: boolean) => invoke<void>("set_allow_lan", { enabled }),
  autostartStatus: () => invoke<boolean>("autostart_status"),
  autostartSet: (enable: boolean) => invoke<void>("autostart_set", { enable }),
  coreVersion: () => invoke<string>("core_version"),
  proxiesSnapshot: () => invoke<GroupView[]>("proxies_snapshot"),
  selectProxy: (group: string, name: string) =>
    invoke<void>("select_proxy", { group, name }),
  testGroupDelay: (group: string) =>
    invoke<Record<string, number>>("test_group_delay", { group }),
  testNodeDelay: (node: string) => invoke<number>("test_node_delay", { node }),
  runDiagnosis: () => invoke<CheckResult[]>("run_diagnosis"),
  currentOutbound: () => invoke<OutboundInfo>("current_outbound"),
  closeConnection: (id: string) => invoke<void>("close_connection", { id }),
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
    listen("traffic://data", (e) =>
      cb(parsePayload<{ up: number; down: number }>(e.payload)),
    ),
  onConnections: (cb: (s: ConnSnapshot) => void) =>
    listen("connections://data", (e) => cb(parsePayload<ConnSnapshot>(e.payload))),
  onLogs: (cb: (entries: LogEntry[]) => void) =>
    listen("logs://data", (e) => cb(parsePayload<LogEntry[]>(e.payload))),
  onTheme: (cb: (t: UiSettings["theme"]) => void) =>
    listen<UiSettings["theme"]>("ui://theme", (e) => cb(e.payload)),
  onLang: (cb: (l: string) => void) =>
    listen<string>("ui://lang", (e) => cb(e.payload)),
  onProfileImported: (cb: (url: string) => void) =>
    listen<string>("profile://imported", (e) => cb(e.payload)),
  onProfileImportFailed: (cb: (msg: string) => void) =>
    listen<string>("profile://import-failed", (e) => cb(e.payload)),
};
