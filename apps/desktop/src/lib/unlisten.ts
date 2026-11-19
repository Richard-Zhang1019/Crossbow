/**
 * #562：统一退订收尾。unlisten promise 的失败来源只有通道已关闭一种，
 * 无补救动作，静默——此前各处裸 `.then(off => off())` 会产生未处理
 * rejection（App/Home cleanup 及各事件注册处统一走本函数）。
 */
export function unsubscribe(promise: Promise<() => void>): void {
  void promise.then((off) => off()).catch(() => {});
}
