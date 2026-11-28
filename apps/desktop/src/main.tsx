import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import "./index.css";
import "@/layouts/Layout.css";
import { isNotificationWindow, NotificationProvider, NotificationWindow } from "@/notifications/NotificationProvider";
if (isNotificationWindow) document.documentElement.classList.add("notification-window");
// macOS 原生质感：标题栏条与纸面沟槽透出窗口材质（Rust 侧 set_effects）。
if (!isNotificationWindow && navigator.userAgent.includes("Macintosh")) {
  document.documentElement.classList.add("is-macos");
}

// ---------- 黑匣子：白屏类问题定位 ----------
// 全局错误与心跳写入 Rust 侧 webview.log：
// - 日志里出现 error/rejection → React/JS 崩溃，可直接看到堆栈
// - 心跳停止且无 error → webview 内容进程死亡（过载/WebKit 崩溃）
function report(kind: string, msg: string) {
  try {
    const t = (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ as
      | { invoke: (cmd: string, args: object) => Promise<unknown> }
      | undefined;
    t?.invoke("diag_log", { line: `[${kind}] ${msg}`.slice(0, 2000) }).catch(
      () => {},
    );
  } catch {
    /* 忽略上报失败 */
  }
}
window.addEventListener("error", (e) =>
  report("error", `${e.message} @ ${e.filename}:${e.lineno}:${e.colno}`),
);
window.addEventListener("unhandledrejection", (e) => {
  const r = e.reason;
  report("rejection", r && r.stack ? String(r.stack) : String(r));
});
setInterval(() => report("heartbeat", "alive"), 2000);

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    {isNotificationWindow ? <NotificationWindow /> : <NotificationProvider><App /></NotificationProvider>}
  </React.StrictMode>,
);
