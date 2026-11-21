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

export interface LanAddress {
  interface: string;
  address: string;
}

export interface CoreStatus {
  state: CoreState;
  message?: string;
}

export interface FaultEvidenceProbe {
  class: string;
  httpStatus: number | null;
}

export interface FaultEvidenceLayer {
  primary: FaultEvidenceProbe;
  review: FaultEvidenceProbe | null;
}

export interface FaultEvidenceRecovery {
  atMs: number;
  proxyHttps: FaultEvidenceLayer;
}

export interface FaultEvidence {
  mode: string;
  startedAtMs: number;
  resolvedAtMs: number | null;
  recovery: FaultEvidenceRecovery | null;
  superseded: boolean;
  legacy: boolean;
  profileId: string | null;
  mixedPort: number;
  sysproxyObserved: string[];
  mixedPortOk: boolean;
  controller: FaultEvidenceProbe;
  directPath: string;
  directCn: FaultEvidenceLayer;
  directGlobal: FaultEvidenceLayer;
  proxyHttps: FaultEvidenceLayer;
}

export interface FaultEvidenceHistory {
  latest: FaultEvidence;
  previous: FaultEvidence | null;
}

export interface SysProxyStatus {
  enabled: boolean;
  port: number;
}

/** SSID 网络场景状态（#566）：规则表 + 当前网络 + 生效场景。 */
export type LocationPermission =
  | "authorized"
  | "denied"
  | "not_determined"
  | "unsupported";

export interface ScenesStatus {
  supported?: boolean;
  networkState?: "available" | "disconnected" | "unavailable";
  rules: Record<string, string>;
  /** 当前关联的 Wi-Fi；null = 无 Wi-Fi 或定位权限缺失。 */
  currentSsid: string | null;
  activeScene: [string, string] | null;
  /** 定位授权状态：not_determined 时展示请求入口，denied 引导系统设置。 */
  location: LocationPermission;
}

export interface PackagePreview {
  id: string;
  report: {
    added_profiles: number;
    replaced_profiles: number;
    added_overrides: number;
    replaced_overrides: number;
    selects_profile: boolean;
  };
}

export interface UiSettings {
  theme: "system" | "light" | "dark";
  lang: string;
  lightweight_close: boolean;
  deep_link_import_confirm: boolean;
}

export interface EngineConfigView {
  engine: "mihomo" | "singbox";
  mixed_port: number;
  allow_lan: boolean;
  flag_emoji: boolean;
  unified_delay: boolean;
  linux_wireguard_coexist: boolean;
  core_installed: boolean;
}

/// #567 本地流量统计：单行（key = 域名/出口节点/日期）与查询结果。
export interface TrafficStatRow {
  key: string;
  up: number;
  down: number;
}

export interface TrafficStatsReport {
  domains: TrafficStatRow[];
  nodes: TrafficStatRow[];
  daily: TrafficStatRow[];
}

/** 一条按进程分流规则（#570）：进程名 → 策略目标。 */
export interface ProcessRule {
  id: string;
  process: string;
  target: string;
}

export interface ProcessRuleStatus {
  rules: ProcessRule[];
  /** 当前订阅可用目标：DIRECT/REJECT + proxy-groups 名。 */
  targets: string[];
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

export interface DiagnosticReport {
  rows: CheckResult[];
  context: {
    os: string;
    os_version: string | null;
    engine: "mihomo" | "sing-box";
    core_state: "running" | "starting" | "stopped" | "crashed";
    core_version: string | null;
    proxy: "configured" | "off" | "mismatch" | "unknown" | "unsupported";
    tun: "ready" | "off" | "inactive" | "unknown";
    mixed_port: number;
  };
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

export interface NodeDelayOutcome {
  class: "ok" | "timeout" | "controller" | "failed" | "cancelled" | "skipped";
  delayMs: number | null;
  reviewed: boolean;
  target: string;
  testedAtMs: number;
  sampleCount: number;
  detail: string;
}

export interface NodeDelayContext {
  revision: number;
  pid: number;
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

// #563：仅保留语义 id，文案统一走 i18n（mode.*）——内置中文 label 是
// 无人消费的死数据，误导后来者以为 label 在用。
export const CORE_MODES = ["direct", "rule", "global"] as const;

export type CoreMode = (typeof CORE_MODES)[number];

export interface CoreBinaryInfo {
  path: string;
  source: "builtin" | "data" | "missing";
  version: string;
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
  kind: "tunnel" | "service" | "core" | "sysproxy-lost";
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
  openFeedbackPage: () => invoke<void>("open_feedback_page"),
  setUnifiedDelay: (enabled: boolean) => invoke<void>("set_unified_delay", { enabled }),
  setEngine: (engine: "mihomo" | "singbox") => invoke<void>("set_engine", { engine }),
  coreInstall: (engine: "mihomo" | "singbox") =>
    invoke<CoreBinaryInfo>("core_install", { engine }),
  coreBinaryInfoFor: (engine: "mihomo" | "singbox") =>
    invoke<CoreBinaryInfo>("core_binary_info_for", { engine }),
  onProfileUpdated: (cb: (id: string) => void) =>
    listen<string>("profile://updated", (e) => cb(e.payload)),
  onTunStatus: (cb: (enabled: boolean) => void) =>
    listen<boolean>("tun://status", (e) => cb(e.payload)),
  setActiveProfile: (id: string) => invoke<void>("set_active_profile", { id }),
  removeProfile: (id: string) => invoke<void>("remove_profile", { id }),
  coreStart: async () => {
    try {
      await invoke<void>("core_start");
    } catch (error) {
      // 保留 Error.message 给现有通知调用方，分类字段供首次连接引导使用。
      if (error && typeof error === "object" && "message" in error && typeof error.message === "string") {
        throw Object.assign(new Error(error.message), error);
      }
      throw error;
    }
  },
  coreRestart: () => invoke<void>("core_restart"),
  logsSettings: () =>
    invoke<{ save_enabled: boolean; retention_days: number }>("logs_settings"),
  setLogsSave: (enabled: boolean) => invoke<void>("set_logs_save", { enabled }),
  setLogsRetention: (days: number) => invoke<void>("set_logs_retention", { days }),
  openLogsDir: () => invoke<void>("open_logs_dir"),
  coreStop: () => invoke<void>("core_stop"),
  appVersion: () => getVersion(),
  exportConfigPackage: () => invoke<boolean>("export_config_package"),
  previewConfigPackage: () => invoke<PackagePreview | null>("preview_config_package"),
  cancelConfigPackage: (id: string) => invoke<void>("cancel_config_package", { id }),
  importConfigPackage: (id: string) => invoke<void>("import_config_package", { id }),
  testSite: (id: string) => invoke<SiteResult>("test_site", { id }),
  coreStatus: () => invoke<CoreStatus>("core_status"),
  sysproxyStatus: () => invoke<SysProxyStatus>("sysproxy_status"),
  sysproxyFaultEvidence: () => invoke<FaultEvidenceHistory | null>("sysproxy_fault_evidence"),
  onFaultEvidenceUpdated: (cb: () => void) => listen("sysproxy://fault-updated", cb),
  sysproxyToggle: () => invoke<void>("sysproxy_toggle"),
  coreMode: () => invoke<CoreMode>("core_mode"),
  setCoreMode: (mode: CoreMode) => invoke<void>("set_core_mode", { mode }),
  getUiSettings: () => invoke<UiSettings>("get_ui_settings"),
  getEngineConfig: () => invoke<EngineConfigView>("get_engine_config"),
  setTheme: (theme: UiSettings["theme"]) => invoke<void>("set_theme", { theme }),
  setLang: (lang: string) => invoke<void>("set_lang", { lang }),
  setDeepLinkImportConfirm: (enabled: boolean) =>
    invoke<void>("set_deep_link_import_confirm", { enabled }),
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
  lanAddresses: () => invoke<LanAddress[]>("lan_addresses"),
  autostartStatus: () => invoke<boolean>("autostart_status"),
  autostartSet: (enable: boolean) => invoke<void>("autostart_set", { enable }),
  trafficHistory: () => invoke<{ up: number; down: number }[]>("traffic_history"),
  /** #567：本地流量统计 TopN + 逐日总量（days 含今天，1/7/30） */
  trafficStats: (days: number) =>
    invoke<TrafficStatsReport>("traffic_stats", { days }),
  proxiesSnapshot: () => invoke<GroupView[]>("proxies_snapshot"),
  selectProxy: (group: string, name: string) =>
    invoke<void>("select_proxy", { group, name }),
  resetProxySelection: (group: string) =>
    invoke<void>("reset_proxy_selection", { group }),
  testGroupDelay: (group: string) =>
    invoke<Record<string, number>>("test_group_delay", { group }),
  proxiesTestSnapshot: () => invoke<{ context: NodeDelayContext; groups: GroupView[] } | null>("proxies_test_snapshot"),
  beginNodeDelayTest: (context: NodeDelayContext, nodeCount: number) =>
    invoke<{ id: string; budgetMs: number } | null>("begin_node_delay_test", { context, nodeCount }),
  testNodeDelayStructured: (taskId: string, node: string) =>
    invoke<NodeDelayOutcome>("test_node_delay_structured", { taskId, node }),
  endNodeDelayTest: (taskId: string) => invoke<void>("end_node_delay_test", { taskId }),
  runDiagnosis: (runId: string) => invoke<DiagnosticReport>("run_diagnosis", { runId }),
  openDiagnosisGuide: (url: string) => invoke<void>("open_diagnosis_guide", { url }),
  diagnosisEnableSysproxy: () => invoke<void>("diagnosis_enable_sysproxy"),
  onProxySelectionChanged: (cb: () => void) => listen("proxy://selection-changed", cb),
  onDiagnosisProgress: (cb:(p:DiagnosisProgress)=>void) => listen<DiagnosisProgress>("diagnosis://progress", e=>cb(parsePayload<DiagnosisProgress>(e.payload))),
  profileStatuses: () => invoke<ProfileStatus[]>("profile_statuses"),
  configSnapshots: () => invoke<ConfigSnapshot[]>("config_snapshots"),
  restoreConfigSnapshot: (id:string) => invoke<void>("restore_config_snapshot",{id}),
  redactLogLines: (lines:string[]) => invoke<string>("redact_log_lines",{lines}),
  exportLogLines: (lines:string[]) => invoke<boolean>("export_log_lines",{lines}),
  currentOutbound: () => invoke<OutboundInfo>("current_outbound"),
  closeConnection: (id: string) => invoke<void>("close_connection", { id }),
  // 按进程分流规则（#570）：变更走 commit_config_change（内核热重载）。
  processRules: () => invoke<ProcessRuleStatus>("process_rules_get"),
  setProcessRule: (id: string | null, process: string, target: string) =>
    invoke<void>("process_rule_set", { id: id ?? "", process, target }),
  removeProcessRule: (id: string) => invoke<void>("process_rule_remove", { id }),
  // traffic/connections 是多页面共享通道：subscriber 标识页面实例，
  // 后端按订阅方登记/移除，最后一个订阅方离开才真正停流（#555）。
  subscribeTraffic: (subscriber: string) =>
    invoke<void>("subscribe_traffic", { subscriber }),
  unsubscribeTraffic: (subscriber: string) =>
    invoke<void>("unsubscribe_traffic", { subscriber }),
  subscribeConnections: (subscriber: string, includeProcesses = false) =>
    invoke<void>("subscribe_connections", { subscriber, includeProcesses }),
  unsubscribeConnections: (subscriber: string) =>
    invoke<void>("unsubscribe_connections", { subscriber }),
  subscribeLogs: (subscriber: string) => invoke<LogSnapshot>("subscribe_logs", { subscriber }),
  clearLogs: () => invoke<LogSnapshot>("clear_logs"),
  unsubscribeLogs: (subscriber: string) => invoke<void>("unsubscribe_logs", { subscriber }),
  onCoreStatus: (cb: (s: CoreStatus) => void) =>
    listen<CoreStatus>("core://status", (e) => cb(e.payload)),
  onProfileSwitched: (cb: (id: string) => void) =>
    listen<string>("profile://switched", (e) => cb(parsePayload<string>(e.payload))),
  onSysproxyStatus: (cb: (s: SysProxyStatus) => void) =>
    listen<SysProxyStatus>("sysproxy://status", (e) => cb(e.payload)),
  // SSID 网络场景（#566）：规则存 store，变更即评估生效。
  networkScenes: () => invoke<ScenesStatus>("network_scenes_get"),
  setNetworkScene: (ssid: string, action: "direct" | "proxy") =>
    invoke<void>("network_scene_set", { ssid, action }),
  removeNetworkScene: (ssid: string) =>
    invoke<void>("network_scene_remove", { ssid }),
  requestSceneLocation: () =>
    invoke<LocationPermission>("network_scene_request_location"),
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
};
