import { useLayoutEffect, useRef, useState, type ReactNode } from "react";

/** One measured viewport owns both axes and the sticky header. */
export default function VirtualList<T>({
  items,
  rowHeight,
  render,
  header,
  empty,
  minWidth = 0,
}: {
  items: T[];
  rowHeight: number;
  render: (item: T, index: number) => ReactNode;
  header?: ReactNode;
  empty?: ReactNode;
  minWidth?: number;
}) {
  const viewport = useRef<HTMLDivElement>(null);
  const heading = useRef<HTMLDivElement>(null);
  const [size, setSize] = useState({ height: 0, header: 0 });
  const [scrollTop, setScrollTop] = useState(0);
  useLayoutEffect(() => {
    const measure = () =>
      setSize({
        height: viewport.current?.clientHeight ?? 0,
        header: heading.current?.offsetHeight ?? 0,
      });
    const observer = new ResizeObserver(measure);
    if (viewport.current) observer.observe(viewport.current);
    if (heading.current) observer.observe(heading.current);
    measure();
    return () => observer.disconnect();
  }, []);
  // A shorter filtered result must not keep a blank, stale virtual window.
  const maximum = Math.max(
    0,
    items.length * rowHeight - Math.max(0, size.height - size.header),
  );
  const effectiveScroll = Math.min(scrollTop, maximum);
  const start = Math.max(
    0,
    Math.floor(Math.max(0, effectiveScroll - size.header) / rowHeight) - 5,
  );
  const visible =
    Math.ceil(Math.max(0, size.height - size.header) / rowHeight) + 10;
  return (
    <div
      ref={viewport}
      className="cb-table-viewport"
      onScroll={(e) => setScrollTop(e.currentTarget.scrollTop)}
    >
      <div style={{ minWidth, minHeight: "100%" }}>
        {header && (
          <div ref={heading} className="cb-table-heading">
            {header}
          </div>
        )}
        {items.length ? (
          <div
            style={{ height: items.length * rowHeight, position: "relative" }}
          >
            <div
              style={{
                position: "absolute",
                top: start * rowHeight,
                left: 0,
                right: 0,
              }}
            >
              {items
                .slice(start, start + visible)
                .map((item, index) => render(item, start + index))}
            </div>
          </div>
        ) : (
          empty
        )}
      </div>
    </div>
  );
}
