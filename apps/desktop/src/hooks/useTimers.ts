import { useCallback, useLayoutEffect, useRef } from "react";

/** Schedule one timeout at a time, with explicit cancellation and unmount cleanup. */
export function useTimeout() {
  const handle = useRef<number | undefined>(undefined);

  const cancel = useCallback(() => {
    if (handle.current === undefined) return;
    window.clearTimeout(handle.current);
    handle.current = undefined;
  }, []);
  useLayoutEffect(() => cancel, [cancel]);

  const schedule = useCallback((fn: () => void, ms: number) => {
    cancel();
    handle.current = window.setTimeout(() => {
      handle.current = undefined;
      fn();
    }, ms);
  }, [cancel]);

  return { schedule, cancel };
}

export function useInterval(fn: () => void, ms: number | null): void {
  const latest = useRef(fn);
  useLayoutEffect(() => {
    latest.current = fn;
  }, [fn]);
  useLayoutEffect(() => {
    if (ms === null) return;
    const timer = window.setInterval(() => latest.current(), ms);
    return () => window.clearInterval(timer);
  }, [ms]);
}
