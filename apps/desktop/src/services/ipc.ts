import { getVersion } from "@tauri-apps/api/app";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

/** 与 Rust 侧对齐的类型。 */
export type OverrideKind = "merge" | "script";

export interface OverrideDef {
  id: string;
  name: string;
  kind: OverrideKind;
  enabled: boolean;
  content: string;
  last_error?: string | null;
}

export interface UpdateOutcome {
  ok: boolean;
  changes?: { added: number; removed: number; changed: number } | null;
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

export interface SiteResult {
  id: string;
  status: "ok" | "restricted" | "http_error" | "error";
  http_status: number | null;
  latency_ms: number | null;
  error: string | null;
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
  engine: "mihomo" | "singbox";
  mixed_port: number;
  allow_lan: boolean;
  flag_emoji: boolean;
  linux_wireguard_coexist: boolean;
  core_installed: boolean;
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
  process_path?: string;
  elapsed_ms: number | null;
}

export interface ConnSnapshot {
  rows: ConnRow[];
  upload_total: number | null;
  download_total: number | null;
  memory: number | null;
  truncated?: number;
}

export interface LogEntry {
  type: string;
  payload: string;
}

export interface CachedLogEntry extends LogEntry {
  sequence: number;
}

export interface LogSnapshot {
  entries: CachedLogEntry[];
  sequence: number;
  connected: boolean;
  generation: number;
}

export interface CheckResult {
  id: string;
  ok: boolean;
  skipped: boolean;
  detail: string;
  fix: string | null;
}

export interface DiagnosisProgress { run_id: string; id: string; result: CheckResult | null }
export interface ProfileStatus { id:string; last_success:number|null; next_update:number|null; retry:boolean; failure:string|null; failure_at?:number|null; using_previous:boolean; changes:{added:number;removed:number;changed:number}|null }
export interface ConfigSnapshot {id:string;at:number;profiles:number;overrides:number;active:string|null}

export interface OutboundInfo {
  mode: string;
  node: string;
  delay_ms: number | null;
  chain: string[];
}

/** Windows 的 WebView2 不渲染地区旗标 emoji（退化为两字母代码 JP/HK）。
 *  显示层剥掉名称开头的旗标序列与其后分隔符；API 调用仍用原始名称。 */
export const IS_WINDOWS = navigator.userAgent.includes("Windows");
const LEADING_FLAG_RE = /^(?:\p{Regional_Indicator}{2})+\s*[|｜]?\s*/u;

export function displayNodeName(name: string): string {
  if (!IS_WINDOWS) return name;
  const stripped = name.replace(LEADING_FLAG_RE, "").trim();
  return stripped || name;
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
  fixed?: string | null;
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

export interface CoexistFinding {
  kind: "tunnel" | "service" | "core";
  detail: string;
}

export interface UpdateState {
  version: string;
  notes: string | null;
  downloaded: boolean;
  progress: [number, number] | null;
}

export interface PlatformCapabilities {
  system_proxy: boolean;
  tun: boolean;
  app_update: boolean;
}

export const ipc = {
  platformCapabilities: () => invoke<PlatformCapabilities>("platform_capabilities"),
  onUpdateState: (cb: (s: UpdateState | null) => void) =>
    listen<UpdateState | null>("update://state", (e) => cb(parsePayload<UpdateState | null>(e.payload))),
  updateStateGet: () => invoke<UpdateState | null>("update_state_get"),
  updateSettingsGet: () =>
    invoke<{ auto_check: boolean; auto_download: boolean }>("update_settings_get"),
  setUpdateAutoCheck: (enabled: boolean) => invoke<void>("set_update_auto_check", { enabled }),
  setUpdateAutoDownload: (enabled: boolean) =>
    invoke<void>("set_update_auto_download", { enabled }),
  updateCheckNow: () => invoke<string | null>("update_check_now"),
  updateStartDownload: () => invoke<void>("update_start_download"),
  updateInstall: () => invoke<void>("update_install"),
  coexistDetect: () => invoke<CoexistFinding[]>("coexist_detect"),
  onGoto: (cb: (page: string) => void) =>
    listen<string>("ui://goto", (e) => cb(parsePayload<string>(e.payload))),

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
  onInstallProgress: (cb: (p: InstallProgress) => void) =>
    listen<InstallProgress>("core://install-progress", (e) => cb(e.payload)),

  onEngineChanged: (cb: (engine: string) => void) =>
    listen<string>("engine://changed", (e) => cb(e.payload)),

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
  listOverrides: () => invoke<OverrideDef[]>("list_overrides"),
  createOverride: (name: string, kind: OverrideKind) =>
    invoke<OverrideDef>("create_override", { name, kind }),
  updateOverride: (id: string, content?: string, name?: string) =>
    invoke<void>("update_override", { id, content: content ?? null, name: name ?? null }),
  removeOverride: (id: string) => invoke<void>("remove_override", { id }),
  toggleOverrideBinding: (profileId: string, overrideId: string, enabled: boolean) =>
    invoke<void>("toggle_override_binding", { profileId, overrideId, enabled }),
  setOverrideEnabled: (id: string, enabled: boolean) =>
    invoke<void>("set_override_enabled", { id, enabled }),
  validateOverrides: () => invoke<void>("validate_overrides"),
  applyOverrides: () => invoke<void>("apply_overrides"),
  setFlagEmoji: (enabled: boolean) => invoke<void>("set_flag_emoji", { enabled }),
  setEngine: (engine: "mihomo" | "singbox") => invoke<void>("set_engine", { engine }),
  coreInstall: (engine: "mihomo" | "singbox") =>
    invoke<CoreBinaryInfo>("core_install", { engine }),
  coreBinaryInfoFor: (engine: "mihomo" | "singbox") =>
    invoke<CoreBinaryInfo>("core_binary_info_for", { engine }),
  onProfileUpdated: (cb: (id: string) => void) =>
    listen<string>("profile://updated", (e) => cb(e.payload)),
  onSafeMode: (cb: (msg: string) => void) =>
    listen<string>("core://safe-mode", (e) => cb(e.payload)),
  onTunStatus: (cb: (enabled: boolean) => void) =>
    listen<boolean>("tun://status", (e) => cb(e.payload)),
  onTunHealthNotice: (cb: (disabled: boolean) => void) =>
    listen<boolean>("tun://health-notice", (e) => cb(e.payload)),
  setActiveProfile: (id: string) => invoke<void>("set_active_profile", { id }),
  removeProfile: (id: string) => invoke<void>("remove_profile", { id }),
  coreStart: () => invoke<void>("core_start"),
  coreRestart: () => invoke<void>("core_restart"),
  logsSettings: () =>
    invoke<{ save_enabled: boolean; retention_days: number }>("logs_settings"),
  setLogsSave: (enabled: boolean) => invoke<void>("set_logs_save", { enabled }),
  setLogsRetention: (days: number) => invoke<void>("set_logs_retention", { days }),
  openLogsDir: () => invoke<void>("open_logs_dir"),
  coreStop: () => invoke<void>("core_stop"),
  appVersion: () => getVersion(),
  testSite: (id: string) => invoke<SiteResult>("test_site", { id }),
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
  getActivityReporting: () => invoke<boolean>("get_activity_reporting"),
  setActivityReporting: (enabled: boolean) =>
    invoke<void>("set_activity_reporting", { enabled }),
  setMixedPort: (port: number) => invoke<void>("set_mixed_port", { port }),
  tunStatus: () =>
    invoke<{ enabled: boolean; ready: boolean; coreRunning: boolean }>("tun_status"),
  setTun: (enable: boolean) => invoke<void>("set_tun", { enable }),
  setLinuxWireguardCoexist: (enabled: boolean) =>
    invoke<void>("set_linux_wireguard_coexist", { enabled }),
  setAllowLan: (enabled: boolean) => invoke<void>("set_allow_lan", { enabled }),
  autostartStatus: () => invoke<boolean>("autostart_status"),
  autostartSet: (enable: boolean) => invoke<void>("autostart_set", { enable }),
  coreVersion: () => invoke<string>("core_version"),
  trafficHistory: () => invoke<{ up: number; down: number }[]>("traffic_history"),
  proxiesSnapshot: () => invoke<GroupView[]>("proxies_snapshot"),
  selectProxy: (group: string, name: string) =>
    invoke<void>("select_proxy", { group, name }),
  resetProxySelection: (group: string) =>
    invoke<void>("reset_proxy_selection", { group }),
  testGroupDelay: (group: string) =>
    invoke<Record<string, number>>("test_group_delay", { group }),
  testNodeDelay: (node: string) => invoke<number>("test_node_delay", { node }),
  runDiagnosis: (runId: string) => invoke<CheckResult[]>("run_diagnosis", { runId }),
  onDiagnosisProgress: (cb:(p:DiagnosisProgress)=>void) => listen<DiagnosisProgress>("diagnosis://progress", e=>cb(parsePayload<DiagnosisProgress>(e.payload))),
  profileStatuses: () => invoke<ProfileStatus[]>("profile_statuses"),
  configSnapshots: () => invoke<ConfigSnapshot[]>("config_snapshots"),
  restoreConfigSnapshot: (id:string) => invoke<void>("restore_config_snapshot",{id}),
  redactLogLines: (lines:string[]) => invoke<string>("redact_log_lines",{lines}),
  exportLogLines: (lines:string[]) => invoke<boolean>("export_log_lines",{lines}),
  currentOutbound: () => invoke<OutboundInfo>("current_outbound"),
  closeConnection: (id: string) => invoke<void>("close_connection", { id }),
  subscribeTraffic: () => invoke<void>("subscribe_traffic"),
  unsubscribeTraffic: () => invoke<void>("unsubscribe_traffic"),
  subscribeConnections: (includeProcesses = false) =>
    invoke<void>("subscribe_connections", { includeProcesses }),
  unsubscribeConnections: () => invoke<void>("unsubscribe_connections"),
  subscribeLogs: (subscriber: string) => invoke<LogSnapshot>("subscribe_logs", { subscriber }),
  clearLogs: () => invoke<LogSnapshot>("clear_logs"),
  unsubscribeLogs: (subscriber: string) => invoke<void>("unsubscribe_logs", { subscriber }),
  onCoreStatus: (cb: (s: CoreStatus) => void) =>
    listen<CoreStatus>("core://status", (e) => cb(e.payload)),
  onProfileSwitched: (cb: (id: string) => void) =>
    listen<string>("profile://switched", (e) => cb(parsePayload<string>(e.payload))),
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
  onLogsStatus: (cb: (status: { connected: boolean; generation: number }) => void) =>
    listen("logs://status", (e) => cb(parsePayload<{ connected: boolean; generation: number }>(e.payload))),
  onLogs: (cb: (entries: CachedLogEntry[]) => void) =>
    listen("logs://data", (e) => cb(parsePayload<CachedLogEntry[]>(e.payload))),
  onTheme: (cb: (t: UiSettings["theme"]) => void) =>
    listen<UiSettings["theme"]>("ui://theme", (e) => cb(e.payload)),
  onLang: (cb: (l: string) => void) =>
    listen<string>("ui://lang", (e) => cb(e.payload)),
  onProfileImported: (cb: (url: string) => void) =>
    listen<string>("profile://imported", (e) => cb(e.payload)),
  onProfileImportFailed: (cb: (msg: string) => void) =>
    listen<string>("profile://import-failed", (e) => cb(e.payload)),
};
