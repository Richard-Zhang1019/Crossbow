import { useEffect, useRef } from "react";

/**
 * #430：页面定时器统一管理。
 *
 * useTimeout：调度型——每次调用自动取消上一次（修复「快速连续复制时旧
 * 定时器提前清掉新 ✓ 徽标」的竞态），组件卸载自动清理。
 *
 * useInterval：latest-ref 型——tick 永远调用最新闭包，ms 变化自动重排；
 * 传 null 暂停。仅用于原本就正确管理的新增场景（存量 setInterval 已有
 * 正确清理与闭包语义的保持不动，避免无谓行为漂移）。
 */
export function useTimeout(): (fn: () => void, ms: number) => void {
  const handle = useRef<number | undefined>(undefined);
  useEffect(
    () => () => {
      if (handle.current !== undefined) window.clearTimeout(handle.current);
    },
    [],
  );
  return (fn: () => void, ms: number) => {
    if (handle.current !== undefined) window.clearTimeout(handle.current);
    handle.current = window.setTimeout(fn, ms);
  };
}

export function useInterval(fn: () => void, ms: number | null): void {
  const latest = useRef(fn);
  latest.current = fn;
  useEffect(() => {
    if (ms === null) return;
    const timer = window.setInterval(() => latest.current(), ms);
    return () => window.clearInterval(timer);
  }, [ms]);
}
