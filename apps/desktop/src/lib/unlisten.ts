/**
 * 页面卸载时统一退订；注册和退订均可能失败，收尾错误不再打扰已卸载页面。
 * 必须在注册时就处理拒绝（下方 subscriptionCleanup 或调用方的 catch），
 * 不能只等到 cleanup 才处理。
 */
export function unsubscribe(promise: Promise<() => void>): void {
  void promise.then((off) => off()).catch(() => {});
}

/**
 * 非关键监听：在注册当下处理拒绝，而非等卸载后才挂 catch。
 * 已有数据的初始读取/轮询继续兜底；返回统一的卸载动作。
 */
export function subscriptionCleanup(promise: Promise<() => void>): () => void {
  void promise.catch(() => {});
  return () => unsubscribe(promise);
}
