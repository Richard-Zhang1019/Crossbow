import { notificationDestination } from "../workflow";
import { invoke } from "@tauri-apps/api/core";
import { currentLang } from "../i18n";

export type NotificationKind =
  | "success"
  | "info"
  | "warning"
  | "error"
  | "progress";
export type NotificationAction =
  | "settings"
  | "diagnostics"
  | "logs"
  | "profiles"
  | "nodes"
  | "retryDownload";
export interface Notification {
  id: string;
  kind: NotificationKind;
  code: string;
  title: string;
  titleKey: string;
  description: string;
  details: string;
  source: string;
  operationId: string;
  dedupeKey: string;
  duration: number | null;
  actions: NotificationAction[];
  progress: number | null;
  createdAt: number;
  updatedAt: number;
  readAt: number | null;
  resolvedAt: number | null;
  count: number;
  dismissed: boolean;
}
export interface NotificationSnapshot {
  revision: number;
  active: Notification[];
  history: Notification[];
  navigateTo: string | null;
}
export type NotificationInput = Partial<Notification> &
  Pick<Notification, "kind">;

const copy = {
  all: ["全部", "All"],
  pending: ["待处理", "Needs attention"],
  markRead: ["全部已读", "Mark all read"],
  unread: ["未读", "unread"],
  session: ["本次会话 · 最多 50 条", "This session · up to 50 records"],
  clear: ["清除已完成", "Clear completed"],
  undo: ["撤销", "Undo"],
  cleared: ["已清除 {n} 条已完成记录", "Cleared {n} completed records"],
  noPending: ["没有待处理的问题", "Nothing needs attention"],
  emptyHint: [
    "重要结果和需要处理的问题会显示在这里。",
    "Important results and issues will appear here.",
  ],
  resolved: ["已恢复", "Recovered"],
  resolvedHint: [
    "此问题已恢复，可在详情中回看原始记录。",
    "This issue has recovered. Original details remain available.",
  ],
  success: ["操作成功", "Completed"],
  info: ["提示", "Information"],
  warning: ["需要注意", "Attention needed"],
  error: ["操作失败", "Operation failed"],
  progress: ["正在处理", "Working"],
  details: ["查看详情", "Details"],
  copy: ["复制详情", "Copy details"],
  copied: ["已复制", "Copied"],
  copyFailed: [
    "复制失败，可选中文本复制",
    "Copy failed. Select the text to copy it.",
  ],
  close: ["关闭通知", "Dismiss notification"],
  history: ["最近通知", "Recent notifications"],
  profiles: ["打开配置", "Open profiles"],
  nodes: ["选择节点", "Pick a node"],
  retryDownload: ["重新下载", "Retry download"],
  empty: ["本次会话暂无通知", "No notifications in this session"],
  settings: ["前往设置", "Open settings"],
  diagnostics: ["打开诊断", "Open diagnostics"],
  logs: ["查看日志", "View logs"],
  failedHint: [
    "操作未完成，请查看详情后重试。",
    "The operation could not finish. Check the details before retrying.",
  ],
  "tray.proxy": ["系统代理切换", "Switch system proxy"],
  "tray.tun": ["TUN 模式切换", "Switch TUN mode"],
  "tray.profile": ["切换配置", "Switch profile"],
  "tray.node": ["节点切换", "Select node"],
  "tray.action": ["托盘操作", "Tray action"],
  "tray.latency": ["延迟测试", "Latency test"],
  "tray.latencyTimeout": ["延迟测试超时", "Latency test timed out"],
  "core.mode": ["切换代理模式", "Switch proxy mode"],
  "stage.downloading": ["正在下载内核…", "Downloading core…"],
  "stage.verifying": ["正在校验内核…", "Verifying core…"],
  "stage.done": ["下载完成", "Download complete"],
  "core.installReady": ["内核已下载", "Core downloaded"],
  "core.install": ["下载安装内核", "Install core"],
  "coexist.resolved": ["代理冲突提示已解除", "Proxy conflict cleared"],
  "profile.updateFailed": [
    "订阅自动更新失败",
    "Automatic subscription update failed",
  ],
  "core.crashed": ["内核运行异常", "Core stopped unexpectedly"],
  "core.recovered": ["内核已恢复运行", "Core is running again"],
  "core.safeMode": ["已进入安全模式", "Safe mode enabled"],
  "tun.disabled": ["TUN 已自动关闭", "TUN turned off automatically"],
  "app.exitBlocked": [
    "退出未完成，应用继续运行",
    "Exit canceled; the app is still running",
  ],
  "tun.externalProxy": [
    "检测到外部系统代理，请手动关闭",
    "External system proxy detected; turn it off manually",
  ],
  "tun.manualRecovery": ["TUN 需要手动恢复", "TUN needs manual recovery"],
  "tun.manualRecovery.hint": [
    "请在设置中手动关闭或重新开启 TUN，并完成系统要求的授权。",
    "Turn TUN off or back on in settings and complete any authorization requested by the system.",
  ],
  "tun.externalProxy.hint": [
    "请在其他代理客户端或系统网络设置中关闭系统代理。处理后提示会自动解除。",
    "Turn off the system proxy in the other proxy app or system network settings. This notice clears when the conflict is resolved.",
  ],
  "tun.wedgeRestart": [
    "TUN 通道无响应，正在重启内核恢复",
    "TUN unresponsive; restarting core to recover",
  ],
  "sysproxy.coreDead": [
    "内核未运行，已关闭系统代理",
    "Core is down; system proxy turned off",
  ],
  "tun.takeoverGiveUp": [
    "TUN 接管异常，已停止自动恢复",
    "TUN takeover abnormal; auto-recovery paused",
  ],
  "tun.disableFailed": ["TUN 自动恢复失败", "TUN recovery failed"],
  "import.done": ["订阅导入成功", "Subscription imported"],
  "import.failed": ["订阅导入失败", "Import failed"],
  "update.available": ["发现新版本", "An update is available"],
  "update.download": ["下载更新", "Download update"],
  "update.ready": [
    "更新已下载，可重启安装",
    "Update downloaded. Ready to install",
  ],
  "update.connecting": ["正在连接下载…", "Connecting to download…"],
  "update.verifying": ["校验更新文件", "Verifying update"],
  "update.verifying.hint": [
    "下载完成，正在校验签名…",
    "Download complete. Verifying signature…",
  ],
  "update.failed": ["更新下载失败", "Update download failed"],
  "safeMode.hint": [
    "覆写导致内核启动失败，已临时禁用全部覆写。请修复后重新应用。",
    "Overrides prevented startup and were disabled temporarily. Fix them before applying again.",
  ],
  "tun.disabled.hint": [
    "连续多次健康检测失败，已关闭 TUN。请检查网络或更换节点。",
    "Repeated health checks failed. TUN was turned off. Check the network or change nodes.",
  ],
} as const;
export function notificationText(
  code: string,
  params?: Record<string, string | number>,
): string {
  const text = copy[code as keyof typeof copy];
  let value: string = text?.[currentLang() === "en" ? 1 : 0] ?? code;
  for (const [key, replacement] of Object.entries(params ?? {}))
    value = value.replace(`{${key}}`, String(replacement));
  return value;
}

// Serialize publication per operation, so a fast result cannot arrive before
// its progress card and overwrite it. Failed IPC must not create an error loop.
const pending = new Map<string, Promise<unknown>>();
function enqueue(
  key: string,
  action: () => Promise<unknown>,
): Promise<unknown> {
  const previous = pending.get(key) ?? Promise.resolve();
  const next = previous
    .catch(() => {})
    .then(action)
    .catch((error) => console.error("notification delivery failed", error));
  pending.set(key, next);
  void next.finally(() => {
    if (pending.get(key) === next) pending.delete(key);
  });
  return next;
}
function publish(input: NotificationInput): Promise<unknown> {
  const key = input.operationId || input.source || input.id || "notification";
  return enqueue(key, () =>
    invoke("notification_publish", {
      notice: {
        id: "",
        code: "",
        title: "",
        titleKey: "",
        description: "",
        details: "",
        source: "ui",
        operationId: "",
        dedupeKey: "",
        duration: null,
        actions: [],
        progress: null,
        ...input,
      },
    }),
  );
}
export interface TaskFeedback {
  feedback?: "task" | "error-only";
  titleKey?: string;
  /** 成功通知附带的跳转动作（条 7：导入成功 → 选节点） */
  nextActions?: NotificationAction[];
}
let attempt = 0;
export const notify = {
  resolve(source: string) {
    return enqueue(source, () => invoke("notification_resolve", { source }));
  },
  publish,
  dismiss(operationId: string) {
    return enqueue(operationId, () =>
      invoke("notification_dismiss", { id: operationId }),
    );
  },
  start(source: string, title: string, description = "") {
    return publish({
      kind: "progress",
      title,
      description,
      source,
      operationId: source,
    });
  },
  success(description: string, source: string, title = "") {
    publish({
      kind: "success",
      title,
      description,
      source,
      operationId: source,
    });
  },
  info(description: string, source: string) {
    publish({ kind: "info", description, source, operationId: source });
  },
  error(error: unknown, source: string, title = "") {
    publish({
      kind: "error",
      title,
      details: String(error),
      source,
      operationId: source,
      actions: notificationDestination(source),
    });
  },
  async run<T>(
    source: string,
    title: string,
    task: () => Promise<T>,
    success?: string,
    options: TaskFeedback = {},
  ): Promise<{ ok: true; value: T } | { ok: false }> {
    const operationId = `${source}:${Date.now()}:${++attempt}`;
    const metadata = {
      source,
      operationId,
      dedupeKey: source,
      titleKey: options.titleKey ?? "",
    };
    // Short actions should not flash a loading card. Local controls already
    // provide feedback for settings, selections and other direct manipulation.
    const timer =
      options.feedback === "error-only"
        ? undefined
        : window.setTimeout(() => {
            void publish({
              ...metadata,
              kind: "progress",
              title: options.titleKey ? "" : title,
            });
          }, 800);
    try {
      const value = await task();
      window.clearTimeout(timer);
      if (options.feedback === "error-only") await notify.resolve(source);
      else
        await publish({
          ...metadata,
          kind: "success",
          title: "",
          description: success ?? title,
          actions: options.nextActions ?? [],
        });
      return { ok: true, value };
    } catch (error) {
      window.clearTimeout(timer);
      await publish({
        ...metadata,
        kind: "error",
        title: options.titleKey ? "" : title,
        details: String(error),
        actions: notificationDestination(source),
      });
      return { ok: false };
    }
  },
};
