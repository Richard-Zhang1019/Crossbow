const ROW_GAP = 8;
// 32px pager plus its separation from the cards.
const PAGER_SPACE = 32 + ROW_GAP;

/** Reserve the pager only when needed, then fit the remaining minimum-size rows. */
export function fitGrid(width: number, height: number, length: number, minWidth: number, maxColumns: number, rowHeight: number, maxRows = Infinity) {
  const columns = minWidth ? Math.max(1, Math.min(maxColumns, Math.floor((width + 10) / (minWidth + 10)))) : 1;
  const fitRows = (available: number) => Math.max(1, Math.min(maxRows, Math.floor((available + ROW_GAP) / (rowHeight + ROW_GAP))));
  const needsPager = length > columns * fitRows(height);
  const rows = Math.min(fitRows(height - (needsPager ? PAGER_SPACE : 0)), Math.max(1, Math.ceil(length / columns)));
  const count = rows * columns;
  return { columns, rows, count, pages: Math.max(1, Math.ceil(length / count)) };
}

/** A short page keeps compact cards. Full-height pages share the same row
 * height, even when the final row has unoccupied columns. */
export function fitRowHeight(height: number, minimum: number, maximum: number, rows: number, pages: number, visibleRows: number) {
  if (visibleRows < 2 || visibleRows < rows) return minimum;
  const available = height - (pages > 1 ? PAGER_SPACE : 0) - (rows - 1) * ROW_GAP;
  return Math.max(minimum, Math.min(maximum, available / rows));
}
