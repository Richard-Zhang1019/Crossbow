import { useState, type ReactNode } from "react";

/**
 * 极简虚拟滚动列表：固定行高 + 窗口化渲染。
 * 上下各多渲染 5 行缓冲，滚动时按 scrollTop 计算窗口。
 */
export default function VirtualList<T>({
  items,
  rowHeight,
  height,
  render,
}: {
  items: T[];
  rowHeight: number;
  height: number;
  render: (item: T, index: number) => ReactNode;
}) {
  const [scrollTop, setScrollTop] = useState(0);
  const start = Math.max(0, Math.floor(scrollTop / rowHeight) - 5);
  const visible = Math.ceil(height / rowHeight) + 10;
  const slice = items.slice(start, start + visible);

  return (
    <div
      style={{ height, overflowY: "auto" }}
      onScroll={(e) => setScrollTop(e.currentTarget.scrollTop)}
    >
      <div style={{ height: items.length * rowHeight, position: "relative" }}>
        <div style={{ position: "absolute", top: start * rowHeight, left: 0, right: 0 }}>
          {slice.map((item, i) => render(item, start + i))}
        </div>
      </div>
    </div>
  );
}
