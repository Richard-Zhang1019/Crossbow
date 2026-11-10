/** Keep backend cumulative counters distinct from rate samples (bytes/second). */
export function cumulativeTotals(snapshot: { upload_total?: number | null; download_total?: number | null }) {
  const counter = (n: number | null | undefined) => typeof n === 'number' && Number.isFinite(n) && n >= 0 ? n : null;
  return { up: counter(snapshot.upload_total), down: counter(snapshot.download_total) };
}

/** An older request must never overwrite a newer event or refresh result. */
export function createReadGate() {
  const versions = new Map<string, number>();
  const begin = (key: string) => { const next = (versions.get(key) ?? 0) + 1; versions.set(key, next); return next; };
  return { begin, invalidate: begin, current: (key: string, version: number) => versions.get(key) === version };
}

export function parsePort(value: string): number | null {
  const text = value.trim();
  if (!/^\d+$/.test(text)) return null;
  const port = Number(text);
  return Number.isSafeInteger(port) && port >= 1024 && port <= 65535 ? port : null;
}
