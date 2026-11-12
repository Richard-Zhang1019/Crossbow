import { invoke as baseInvoke } from "../../2026-11-05-overview-height/v02/mock-tauri";
export * from "../../2026-11-05-overview-height/v02/mock-tauri";

const params = new URLSearchParams(location.search);
const state = params.get("update");

export async function invoke(command: string, args: Record<string, unknown> = {}) {
  if (command === "update_state_get") {
    if (!state || state === "unsupported") return null;
    return {
      version: "0.13.8",
      notes: null,
      downloaded: state === "ready",
      progress: state === "progress" ? [42, 100] : state === "unknown" ? [42, 0] : null,
    };
  }
  if (command === "platform_capabilities" && state === "unsupported") {
    return { system_proxy: true, tun: true, app_update: false };
  }
  return baseInvoke(command, args);
}
