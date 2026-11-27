// 后端在「内核重启中」等待超时后返回的错误前缀（#666）。
export const CORE_RESTARTING_MARKER = "core:restarting";

/** 判断错误是否来自内核重启窗口；这类失败只提示，不进待处理。 */
export function isCoreRestartingError(error: unknown): boolean {
  return String(error).includes(CORE_RESTARTING_MARKER);
}
