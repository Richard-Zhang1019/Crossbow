// Browser-only fixture. Every IPC call is intercepted before loading the app.
import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";

const options = new URLSearchParams(location.search);
const initiallyConfigured = options.has("configured") || options.has("stopped");
const key = `first-connection-preview:${location.search}`;
const state = JSON.parse(sessionStorage.getItem(key) ?? "null") ?? {
  configured: initiallyConfigured,
  installed: !options.has("missing"),
  core: options.has("configured") ? "Running" : "Stopped",
  proxy: options.has("configured"),
  tun: false,
  port: 7898,
  downloadFails: options.has("missing"),
  authCancelled: false,
  siteFails: options.has("probeFail"),
};
const save = () => sessionStorage.setItem(key, JSON.stringify(state));
const pause = (ms: number) => new Promise(resolve => setTimeout(resolve, ms));
const emit = (event: string, payload: unknown) => {
  const tauri = window as Window & { __TAURI_INTERNALS__: {
    invoke: (command: string, args: unknown) => Promise<unknown>;
  } };
  return tauri.__TAURI_INTERNALS__.invoke("plugin:event|emit", { event, payload });
};
const profile = { id: "demo", name: "Demo", kind: "local", override_ids: [], update_interval_min: 0 };
let history: Array<Record<string, unknown>> = [];
let revision = 0;
let navigateTo: string | null = null;
const snapshot = () => ({ revision, active: [], history, navigateTo });
const notify = async (value: Record<string, unknown>) => {
  const notice = {
    id: String(++revision), code: "", title: "", titleKey: "", description: "", details: "",
    source: "", dedupeKey: "", duration: null, actions: [], progress: null, createdAt: Date.now(),
    updatedAt: Date.now(), readAt: null, resolvedAt: null, count: 1, dismissed: false,
    ...value,
  };
  history = [notice, ...history.filter(n => n.operationId !== value.operationId)];
  await emit("app://notifications", snapshot());
  return notice.id;
};
mockWindows("main");
mockIPC(async (command, args) => {
  const values = args as {
    port: number; enable: boolean; id: string; notice: Record<string, unknown>; action: string;
  };
  switch (command) {
    case "get_ui_settings": return { lang: options.has("en") ? "en" : "zh", theme: options.has("dark") ? "dark" : "light" };
    case "platform_capabilities": return { system_proxy: true, tun: true, app_update: false };
    case "list_profiles": return state.configured ? [profile] : [];
    case "active_profile_id": return state.configured ? profile.id : null;
    case "profile_statuses":
    case "coexist_detect":
    case "traffic_history": return [];
    case "get_engine_config": return { engine: "mihomo", mixed_port: state.port, core_installed: state.installed };
    case "core_binary_info_for": return { source: state.installed ? "builtin" : "missing", version: "mihomo v1.19.32" };
    case "core_version": return "mihomo v1.19.32";
    case "app_version": return "0.14.3-preview";
    case "core_status": return { state: state.core };
    case "core_mode": return "rule";
    case "sysproxy_status": return { enabled: state.proxy, port: state.port };
    case "tun_status": return { enabled: state.tun, ready: true, coreRunning: state.core === "Running" };
    case "current_outbound": return { node: "Demo · Tokyo", chain: ["Demo · Tokyo"], delay_ms: 84 };
    case "import_profile_url":
    case "import_profile_content":
      state.configured = true;
      save();
      return profile;
    case "core_install":
      await notify({ operationId: "core-download", kind: "progress", description: "downloading" });
      await pause(650);
      if (state.downloadFails) {
        state.downloadFails = false;
        save();
        await notify({ operationId: "core-download", kind: "error" });
        throw "Simulated network timeout";
      }
      state.installed = true;
      save();
      await notify({ operationId: "core-download", kind: "success", actions: ["continueSetup"] });
      return {};
    case "core_start":
      if (options.has("busyPort") && state.port === 7898) throw "混合端口 7898 无法绑定（模拟）";
      state.core = "Starting";
      await emit("core://status", { state: state.core });
      await pause(180);
      state.core = "Running";
      save();
      await emit("core://status", { state: state.core });
      return;
    case "set_mixed_port":
      state.port = values.port;
      save();
      return;
    case "set_tun":
      if (options.has("cancelTun") && !state.authCancelled) {
        state.authCancelled = true;
        save();
        await notify({ operationId: "tun-switch", kind: "error" });
        throw "Simulated authorization cancellation";
      }
      state.tun = values.enable;
      save();
      await emit("tun://status", state.tun);
      return;
    case "sysproxy_toggle":
      state.proxy = !state.proxy;
      save();
      await emit("sysproxy://status", { enabled: state.proxy, port: state.port });
      return;
    case "test_site": {
      await pause(180);
      const failed = state.siteFails;
      state.siteFails = false;
      save();
      return { id: values.id, status: failed ? "error" : "ok", http_status: failed ? null : 200, error: failed ? "timeout" : null };
    }
    case "run_diagnosis":
      return {
        context: { os: "macos" },
        rows: [{ id: "core_running", ok: false, skipped: false, detail: "Core stopped (simulated)", fix: "start_core" }],
      };
    case "notification_snapshot": return snapshot();
    case "notification_publish": return notify(values.notice);
    case "notification_action":
      if (values.action === "continueSetup") {
        navigateTo = JSON.stringify({ page: "home", control: "cb-first-connection" });
        ++revision;
        await emit("app://notifications", snapshot());
      }
      return;
    case "notification_ack_navigation":
      navigateTo = null;
      ++revision;
      await emit("app://notifications", snapshot());
      return;
  }
  return null;
}, { shouldMockEvents: true });

void import("../../../../apps/desktop/src/main");
