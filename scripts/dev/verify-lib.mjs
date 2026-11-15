// #511 可复跑桌面验证：共享基础设施。
//
// 三件事：
//  1. startVite —— 起 apps/desktop 的 vite dev（生产组件按原样渲染，无任何测试替身组件）。
//  2. stubSource —— 注入 window.__TAURI_INTERNALS__ 桩：invoke 分发表 + 事件注册表，
//     IPC 夹具由场景通过 window.__cb 控制迟到/失败/事件顺序；transformCallback 真注册
//     回调（@tauri-apps/api listen 的前置条件）；所有数据脱敏，无真实节点/订阅。
//  3. findChrome + launch —— playwright-core 只驱动系统已装浏览器，不下载任何二进制。

import { spawn } from "node:child_process";
import { existsSync, mkdirSync, readdirSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

export const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
export const DESKTOP_DIR = path.join(REPO_ROOT, "apps/desktop");
export const VERIFY_PORT = 5199;
export const VERIFY_URL = `http://127.0.0.1:${VERIFY_PORT}/`;

/** 验证专用版本号：页脚 "Crossbow v…" 是场景的启动完成标记。 */
export const STUB_APP_VERSION = "0.14.4-verify";

export async function startVite() {
  const child = spawn("pnpm", ["exec", "vite", "--port", String(VERIFY_PORT), "--strictPort", "--host", "127.0.0.1"], {
    cwd: DESKTOP_DIR,
    stdio: ["ignore", "pipe", "pipe"],
    env: { ...process.env, BROWSER: "none" },
  });
  let output = "";
  child.stdout.on("data", (d) => { output += d; });
  child.stderr.on("data", (d) => { output += d; });
  const deadline = Date.now() + 30_000;
  while (Date.now() < deadline) {
    if (child.exitCode !== null) break;
    try {
      const res = await fetch(VERIFY_URL);
      if (res.ok) return child;
    } catch { /* not up yet */ }
    await sleep(250);
  }
  child.kill("SIGTERM");
  throw new Error(`vite dev 未就绪：\n${output.slice(-2000)}`);
}

export function stopVite(child) {
  if (child && child.exitCode === null) child.kill("SIGTERM");
}

export const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/** 只用系统已有浏览器（Chrome/Chromium/Edge），或本地曾装过的 playwright 缓存；
 *  绝不触发浏览器下载。 */
export function findChromeExecutable() {
  const candidates = [];
  if (process.env.CB_CHROME_BIN) candidates.push(process.env.CB_CHROME_BIN);
  if (process.platform === "darwin") {
    for (const app of ["Google Chrome", "Chromium", "Microsoft Edge", "Arc"]) {
      candidates.push(`/Applications/${app}.app/Contents/MacOS/${app === "Arc" ? "Arc" : app}`);
    }
  } else if (process.platform === "win32") {
    const pf = process.env["ProgramFiles"] ?? "C:\\Program Files";
    const lf = process.env["LOCALAPPDATA"] ?? "";
    for (const base of [pf, `${pf} (x86)`, lf]) {
      candidates.push(path.join(base, "Google\\Chrome\\Application\\chrome.exe"));
      candidates.push(path.join(base, "Chromium\\Application\\chrome.exe"));
      candidates.push(path.join(base, "Microsoft\\Edge\\Application\\msedge.exe"));
    }
  } else {
    for (const name of ["google-chrome", "google-chrome-stable", "chromium", "chromium-browser", "microsoft-edge"]) {
      candidates.push(name);
    }
  }
  // 本地曾跑过 playwright（含本仓库历史验证）时复用其缓存 chromium。
  const cacheRoot = process.platform === "darwin"
    ? path.join(os.homedir(), "Library/Caches/ms-playwright")
    : process.platform === "win32"
      ? path.join(process.env.LOCALAPPDATA ?? "", "ms-playwright")
      : path.join(os.homedir(), ".cache/ms-playwright");
  if (existsSync(cacheRoot)) {
    for (const entry of readdirSync(cacheRoot)) {
      if (!entry.startsWith("chromium-")) continue;
      if (process.platform === "darwin") {
        candidates.push(path.join(cacheRoot, entry, "chrome-mac/Chromium.app/Contents/MacOS/Chromium"));
        candidates.push(path.join(cacheRoot, entry, "chrome-mac-arm64/Chromium.app/Contents/MacOS/Chromium"));
      } else if (process.platform === "win32") {
        candidates.push(path.join(cacheRoot, entry, "chrome-win/chrome.exe"));
      } else {
        candidates.push(path.join(cacheRoot, entry, "chrome-linux/chrome"));
      }
    }
  }
  for (const c of candidates) {
    if (c.includes("/") || c.includes("\\")) {
      if (existsSync(c)) return c;
    } else {
      const resolved = which(c);
      if (resolved) return resolved;
    }
  }
  throw new Error(
    "未找到可用的浏览器。请安装 Google Chrome，或设置 CB_CHROME_BIN 指向 Chrome/Chromium 可执行文件。\n" +
    "本脚本刻意不下载浏览器二进制。",
  );
}

function which(name) {
  const dirs = (process.env.PATH ?? "").split(path.delimiter);
  for (const dir of dirs) {
    const full = path.join(dir, name);
    if (existsSync(full)) return full;
  }
  return null;
}

export async function launchBrowser(headless) {
  const { chromium } = await import("playwright-core");
  const require = createRequire(import.meta.url);
  require.resolve("playwright-core"); // 失败时报_install 提示而不是 ESM 内部栈
  const args = ["--disable-dev-shm-usage"];
  if (process.env.CI || process.platform === "linux") {
    args.push("--no-sandbox", "--disable-setuid-sandbox");
  }
  return chromium.launch({ executablePath: findChromeExecutable(), headless, args });
}

/** 失败现场（截图 + 控制台 + IPC 调用计数）写到临时目录，绝不进仓库。 */
export function artifactsDir() {
  const dir = path.join(os.tmpdir(), `crossbow-verify-${new Date().toISOString().replace(/[:.]/g, "-")}`);
  mkdirSync(dir, { recursive: true });
  return dir;
}

export function writeEvidence(dir, name, page, extra) {
  const files = [];
  try {
    const shot = path.join(dir, `${name}.png`);
    // 同步 API 不可用于截图；这里只记录路径，截图由调用方异步补写。
    files.push(shot);
  } catch { /* ignore */ }
  writeFileSync(path.join(dir, `${name}.calls.json`), JSON.stringify(extra ?? {}, null, 2));
  return files;
}

/**
 * __TAURI_INTERNALS__ 桩源码（字符串，经 addInitScript 在任何应用脚本前执行）。
 * 场景通过 window.__cb 控制夹具：
 *   __cb.setCommands({ cmd: value | {delayMs, value} | async (args) => result })
 *   __cb.emit("event://name", payload)     —— 按注册表派发给 listen 回调
 *   __cb.calls / __cb.unknown              —— 断言派发次数 / 未注册命令
 */
export function stubSource() {
  return `
(() => {
  "use strict";
  // 启动夹具表（普通 JS 对象，函数夹具合法）。值为 null 也是已注册命令（可空返回）。
  const config = {
    "platform_capabilities": { system_proxy: true, tun: true, app_update: false },
    "plugin:app|version": "${STUB_APP_VERSION}",
    "core_status": { state: "Running" },
    "list_profiles": [
      {
        id: "p-a", name: "验证订阅A", kind: "remote", url: "https://stub.invalid/a", update_interval_min: 0,
        override_ids: [], last_updated: null, traffic: null, last_update: null,
      },
      {
        id: "p-b", name: "验证订阅B", kind: "remote", url: "https://stub.invalid/b", update_interval_min: 0,
        override_ids: [], last_updated: null, traffic: null, last_update: null,
      },
    ],
    "active_profile_id": "p-a",
    "sysproxy_status": { enabled: false, port: 7897 },
    "tun_status": { enabled: false, ready: true, coreRunning: true },
    "core_mode": "rule",
    "get_engine_config": {
      engine: "mihomo", mixed_port: 7897, allow_lan: false, flag_emoji: true,
      linux_wireguard_coexist: false, core_installed: true,
    },
    "core_binary_info_for": { path: "/stub/binaries/mihomo", source: "builtin", version: "mihomo v1.19.32 (stub)" },
    "get_ui_settings": { theme: "system", lang: "zh", lightweight_close: false, deep_link_import_confirm: true },
    "notification_snapshot": { revision: 0, active: [], history: [], navigateTo: null },
    "coexist_detect": [],
    "traffic_history": [],
    "profile_statuses": [],
    "config_snapshots": [],
    "logs_settings": { save_enabled: false, retention_days: 7 },
    "autostart_status": false,
    "get_activity_reporting": false,
    "update_settings_get": { auto_check: true, auto_download: false },
    "update_state_get": null,
    "current_outbound": { mode: "rule", node: "🇭🇰 出口-01", delay_ms: 88, chain: ["🇭🇰 出口-01"] },
    "subscribe_traffic": null,
    "unsubscribe_traffic": null,
    "subscribe_connections": null,
    "unsubscribe_connections": null,
    "sysproxy_fault_evidence": null,
    "test_site": (args) => ({ id: args?.id ?? "", status: "ok", http_status: 200, latency_ms: 45, error: null }),
    "proxies_test_snapshot": null,
    "begin_node_delay_test": () => ({ id: "task-" + (calls.begin_node_delay_test ?? 1), budgetMs: 30000 }),
    "test_node_delay_structured": null,
    "end_node_delay_test": null,
    "run_diagnosis": null,
  };
  const silent = new Set(${JSON.stringify([
    "diag_log",
    "notification_ack_navigation", "notification_mark_read", "notification_dismiss",
    "notification_pause", "notification_action", "notification_resize",
    "notification_restore_completed", "notification_publish", "notification_resolve",
  ])});
  const calls = {};
  const unknown = [];
  const invokeLog = [];
  const registry = new Map();   // handlerId -> { event, fn }
  const byEvent = new Map();    // event -> Set<handlerId>
  let nextId = 100;
  const wait = (ms) => new Promise((r) => setTimeout(r, ms));

  window.__cb = {
    calls, unknown, invokeLog,
    setCommands(overrides) { Object.assign(config, overrides); },
    handlerCount(event) { return (byEvent.get(event) ?? new Set()).size; },
    emit(event, payload) {
      const ids = [...(byEvent.get(event) ?? new Set())];
      let n = 0;
      for (const id of ids) {
        const entry = registry.get(id);
        if (!entry) continue;
        n += 1;
        const fn = entry.fn;
        setTimeout(() => { try { fn({ event, id: String(id), payload }); } catch (e) { console.error("[stub] 事件回调异常", event, e); } }, 0);
      }
      return n;
    },
  };

  window.__TAURI_INTERNALS__ = {
    metadata: {
      currentWindow: { label: "main" },
      currentWebview: { label: "main", windowLabel: "main" },
    },
    transformCallback(fn, once) {
      const id = nextId++;
      window["_" + id] = (...args) => {
        const result = fn(...args);
        if (once) delete window["_" + id];
        return result;
      };
      return id;
    },
    // 与官方 mocks 同面：emit 侧按 id 取回调
    unregisterCallback(id) { delete window["_" + id]; },
    runCallback(id, data) {
      const fn = window["_" + id];
      if (fn) fn(data);
    },
    invoke(cmd, args = {}) {
      calls[cmd] = (calls[cmd] ?? 0) + 1;
      invokeLog.push({ cmd, args, at: Date.now() });
      if (cmd === "plugin:event|listen") {
        const { event, handler } = args;
        registry.set(handler, { event, fn: window["_" + handler] });
        if (!byEvent.has(event)) byEvent.set(event, new Set());
        byEvent.get(event).add(handler);
        return Promise.resolve(handler);
      }
      if (cmd === "plugin:event|unlisten") {
        const { event, eventId } = args;
        unregisterListener(event, eventId);
        return Promise.resolve(null);
      }
      if (silent.has(cmd)) return Promise.resolve(null);
      // 值为 null 也是已注册命令（可空返回），只有完全未登记才算夹具缺口
      if (cmd in config) {
        const v = config[cmd];
        if (typeof v === "function") {
          // 第三个参数 seq 是本次调用的序号（1 起）——夹具在微任务里执行，
          // 不能读 calls 计数，否则并发下全部读到同一偏移
          const seq = calls[cmd];
          return Promise.resolve().then(() => v(args, cmd, seq));
        }
        if (v !== null && v !== undefined && typeof v === "object" && "delayMs" in v) {
          return wait(v.delayMs).then(() => ("value" in v ? v.value : null));
        }
        return Promise.resolve(v);
      }
      unknown.push({ cmd, args, at: Date.now() });
      return Promise.reject(new Error("[verify-stub] 未注册命令: " + cmd));
    },
  };
  function unregisterListener(event, eventId) {
    registry.delete(eventId);
    byEvent.get(event)?.delete(eventId);
  }
  // @tauri-apps/api event._unlisten 先走这里，再发 plugin:event|unlisten
  window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener };
})();
`;
}
