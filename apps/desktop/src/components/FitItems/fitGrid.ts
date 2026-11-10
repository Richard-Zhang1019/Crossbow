/** Reserve the pager only when needed, then fit the remaining minimum-size rows. */
export function fitGrid(width: number, height: number, length: number, minWidth: number, maxColumns: number, rowHeight: number, maxRows = Infinity) {
  const columns = minWidth ? Math.max(1, Math.min(maxColumns, Math.floor((width + 10) / (minWidth + 10)))) : 1;
  const fitRows = (available: number) => Math.max(1, Math.min(maxRows, Math.floor((available + 8) / (rowHeight + 8))));
  const needsPager = length > columns * fitRows(height);
  const rows = Math.min(fitRows(height - (needsPager ? 40 : 0)), Math.max(1, Math.ceil(length / columns)));
  const count = rows * columns;
  return { columns, rows, count, pages: Math.max(1, Math.ceil(length / count)) };
}

/** Single-page lists and incomplete last pages stay compact. Only a full page
 * may use extra room, capped at 1.5× the natural row height. */
export function fitRowHeight(height: number, natural: number, rows: number, pages: number, visibleCount: number, capacity: number) {
  if (pages <= 1 || visibleCount < capacity) return natural;
  const available = (height - 40 - Math.max(0, rows - 1) * 8) / rows;
  return Math.max(natural, Math.min(natural * 1.5, available));
}
