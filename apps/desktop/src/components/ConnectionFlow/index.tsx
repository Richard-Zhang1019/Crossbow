import { useEffect, useLayoutEffect, useRef, useState } from "react";
import type { CSSProperties, ReactNode } from "react";

// Approved v03 design: #165. 840 ms travel, then 2560 ms between packets.
// Traffic frames normally arrive every second. A short grace period avoids
// restarting the CSS timeline for brief zero-rate frames; stale streams expire.
const TRAFFIC_FRESH_MS = 4000;

export function ConnectionFlow({
  enabled,
  lastTrafficAt,
  children,
}: {
  enabled: boolean;
  lastTrafficAt: number;
  children: ReactNode;
}) {
  const root = useRef<HTMLDivElement>(null);
  const [recentTraffic, setRecentTraffic] = useState(false);
  const [hidden, setHidden] = useState(() => document.hidden);
  const [geometry, setGeometry] = useState<CSSProperties>();

  useEffect(() => {
    const remaining = lastTrafficAt + TRAFFIC_FRESH_MS - Date.now();
    if (!enabled || !lastTrafficAt || remaining <= 0) {
      setRecentTraffic(false);
      return;
    }
    setRecentTraffic(true);
    const timer = window.setTimeout(() => setRecentTraffic(false), remaining);
    return () => window.clearTimeout(timer);
  }, [enabled, lastTrafficAt]);

  useEffect(() => {
    const onVisibility = () => setHidden(document.hidden);
    document.addEventListener("visibilitychange", onVisibility);
    return () => document.removeEventListener("visibilitychange", onVisibility);
  }, []);

  useLayoutEffect(() => {
    const element = root.current;
    if (!element) return;
    const segments = element.querySelectorAll<HTMLElement>("[data-route-segment]");
    const [first, second] = Array.from(segments);
    if (!first || !second) return;

    const measure = () => {
      const bounds = element.getBoundingClientRect();
      const a = first.getBoundingClientRect();
      const b = second.getBoundingClientRect();
      const width = b.right - a.left;
      if (a.width <= 0 || b.width <= 0 || width <= 0) {
        setGeometry(undefined);
        return;
      }
      const next = {
        left: a.left - bounds.left,
        top: a.top - bounds.top - 5.5,
        width,
        "--cb-flow-first-end": `${a.width}px`,
        "--cb-flow-second-start": `${b.left - a.left}px`,
      } as CSSProperties;
      setGeometry((current) =>
        JSON.stringify(current) === JSON.stringify(next) ? current : next,
      );
    };
    measure();
    // Only measure on layout changes, never for each animation frame. The
    // actual gaps can vary with language, window size and long node names.
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    for (const child of Array.from(element.children)) observer.observe(child);
    return () => observer.disconnect();
  }, []);

  return (
    <div
      className="cb-route-flow"
      ref={root}
      data-transferring={enabled && recentTraffic}
      data-paused={hidden}
    >
      {children}
      {geometry && (
        <span className="cb-route-motion" style={geometry} aria-hidden="true">
          <i className="cb-route-packet" />
        </span>
      )}
    </div>
  );
}
