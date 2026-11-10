import { ButtonBase } from "@/components/Button";
import { fitGrid, fitRowHeight } from "./fitGrid";
import "./FitItems.css";
import { useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { useT } from "@/i18n/i18n";

/** Paginate finite option lists instead of adding a second page scrollbar. */
export default function FitItems<T>({
  items,
  render,
  rowHeight = 64,
  minWidth = 0,
  maxColumns = 1,
  label,
  stretch = false,
  maxRows,
}: {
  items: T[];
  render: (item: T) => ReactNode;
  rowHeight?: number;
  minWidth?: number;
  maxColumns?: number;
  label: string;
  stretch?: boolean;
  maxRows?: number;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const [size, setSize] = useState({ width: 0, height: 0 });
  const [page, setPage] = useState(0);
  const t = useT();
  useLayoutEffect(() => {
    const update = () => {
      const e = ref.current;
      if (e) setSize({ width: e.clientWidth, height: e.clientHeight });
    };
    const observer = new ResizeObserver(update);
    if (ref.current) observer.observe(ref.current);
    update();
    return () => observer.disconnect();
  }, []);
  const { columns, rows, count, pages } = fitGrid(size.width, size.height, items.length, minWidth, maxColumns, rowHeight, maxRows);
  const active = Math.min(page, pages - 1);
  const pageItems = items.slice(active * count, (active + 1) * count);
  const visibleRows = Math.max(1, Math.ceil(pageItems.length / columns));
  const height = stretch
    ? fitRowHeight(size.height, rowHeight, rows, pages, pageItems.length, count)
    : rowHeight;
  return (
    <div ref={ref} className={`cb-fit-items ${stretch ? "cb-fit-stretch" : ""}`} aria-label={label}>
      <div
        className="cb-fit-cells"
        style={{
          gridTemplateColumns: `repeat(${columns},minmax(0,1fr))`,
          gridAutoRows: height,
          gridTemplateRows: stretch ? `repeat(${visibleRows}, ${height}px)` : undefined,
        }}
      >
        {pageItems.map(render)}
      </div>
      {pages > 1 && (
        <nav className="cb-fit-pager" aria-label={label}>
          <ButtonBase
            disabled={active === 0}
            onClick={() => setPage(active - 1)}
            aria-label={t("studio.previous")}
          >
            ‹
          </ButtonBase>
          <span>
            {active + 1} / {pages}
          </span>
          <ButtonBase
            disabled={active + 1 >= pages}
            onClick={() => setPage(active + 1)}
            aria-label={t("studio.next")}
          >
            ›
          </ButtonBase>
        </nav>
      )}
    </div>
  );
}
