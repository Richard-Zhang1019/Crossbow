import type { CachedLogEntry, LogSnapshot } from "@/services/ipc";

const LIMIT = 2000;

function ordered(entries: CachedLogEntry[], after = 0): CachedLogEntry[] {
  const unique = new Map<number, CachedLogEntry>();
  for (const entry of entries) {
    if (entry.sequence > after) unique.set(entry.sequence, entry);
  }
  return [...unique.values()].sort((a, b) => a.sequence - b.sequence).slice(-LIMIT);
}

/** Join a backend snapshot with events received before its IPC reply.
 * The cursor also fences delayed batches after a user clears the cache.
 */
export class LogHistory {
  private cursor = 0;
  private paused = true;
  private buffered: CachedLogEntry[] = [];

  accept(entries: CachedLogEntry[]): CachedLogEntry[] {
    if (this.paused) {
      this.buffered = ordered([...this.buffered, ...entries], this.cursor);
      return [];
    }
    const fresh = ordered(entries, this.cursor);
    this.cursor = Math.max(this.cursor, fresh.at(-1)?.sequence ?? 0);
    return fresh;
  }

  hydrate(snapshot: LogSnapshot): CachedLogEntry[] {
    const entries = ordered([
      ...snapshot.entries,
      ...this.buffered.filter((entry) => entry.sequence > snapshot.sequence),
    ]);
    this.cursor = Math.max(snapshot.sequence, entries.at(-1)?.sequence ?? 0);
    this.buffered = [];
    this.paused = false;
    return entries;
  }

  pause() {
    this.paused = true;
    this.buffered = [];
  }

  resume(): CachedLogEntry[] {
    const entries = this.buffered;
    this.buffered = [];
    this.paused = false;
    return this.accept(entries);
  }
}
