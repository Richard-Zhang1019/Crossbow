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
  onCoreStatus: (cb: (s: CoreStatus) => void) =>
    listen<CoreStatus>("core://status", (e) => cb(e.payload)),
  onSysproxyStatus: (cb: (s: SysProxyStatus) => void) =>
    listen<SysProxyStatus>("sysproxy://status", (e) => cb(e.payload)),
  onCoreMode: (cb: (m: CoreMode) => void) =>
    listen<CoreMode>("core://mode", (e) => cb(e.payload)),
};
