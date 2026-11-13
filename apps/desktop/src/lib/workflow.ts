import type { ConnRow } from "@/services/ipc";
export const DIAG_STEPS = [
  "core_running",
  "controller",
  "config_renders",
  "listener",
  "sysproxy",
  "proxied_traffic",
  "dns_direct",
] as const;
export function sortConnections(rows: ConnRow[], sort: string) {
  const key =
    sort === "down" ? "down" : sort === "up" ? "up" : sort === "age" ? "elapsed_ms" : null;
  return key
    ? [...rows].sort((a, b) => (b[key] ?? -1) - (a[key] ?? -1) || a.id.localeCompare(b.id))
    : rows;
}
export function connectionDetails(row: ConnRow) {
  return {
    Target: row.host,
    Network: row.network,
    Type: row.kind,
    Rule: row.rule,
    Chain: row.chains,
    Process: row.process_path || row.process,
    ID: row.id,
  };
}
export type NavigationTarget = {
  page: string;
  profileId?: string;
  settingSection?: "network" | "appearance" | "logs" | "app";
  control?: string;
  checkId?: string;
};
export function navigateTo(target: NavigationTarget) {
  window.dispatchEvent(new CustomEvent("cb:goto", { detail: JSON.stringify(target) }));
}

export function parseNavigation(raw: string): NavigationTarget | null {
  try {
    const v = raw.startsWith("{") ? JSON.parse(raw) : { page: raw };
    if (
      !["home", "proxies", "profiles", "connections", "logs", "settings", "diagnostics"].includes(
        v.page,
      )
    )
      return null;
    return {
      page: v.page,
      profileId: typeof v.profileId === "string" ? v.profileId : undefined,
      settingSection: ["network", "appearance", "logs", "app"].includes(v.settingSection)
        ? v.settingSection
        : undefined,
      control:
        typeof v.control === "string" &&
        [
          "mixed-port",
          "cb-port-setting",
          "cb-update-status",
          "cb-tun-control",
          "cb-core-download",
          "cb-log-search",
          "cb-profile-detail",
          "cb-config-recovery",
          "cb-import-profile",
          "cb-first-connection",
        ].includes(v.control)
          ? v.control
          : undefined,
      checkId:
        typeof v.checkId === "string" && [...DIAG_STEPS, "first-problem"].includes(v.checkId)
          ? v.checkId
          : undefined,
    };
  } catch {
    return null;
  }
}

export function notificationDestination(
  source: string,
): Array<"settings" | "profiles" | "logs" | "diagnostics"> {
  if (/(?:copy|export)/.test(source)) return [];
  if (source.startsWith("setting") || source.startsWith("update")) return ["settings"];
  if (source.startsWith("profile") || source.startsWith("snapshot")) return ["profiles"];
  if (source.startsWith("logs")) return ["logs"];
  return ["diagnostics"];
}
