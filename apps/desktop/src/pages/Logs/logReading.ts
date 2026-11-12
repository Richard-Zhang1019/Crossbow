/** A temporary view snapshot; ingestion and its bounded history remain independent. */
export class LogReadingWindow<T extends object> {
  private holds = new Set<"pointer" | "focus">();
  private snapshot: T[] | null = null;
  private ids = new WeakMap<T, number>();
  private nextId = 0;

  hold(kind: "pointer" | "focus", active: boolean, live: T[]): T[] | null {
    if (active) this.holds.add(kind);
    else this.holds.delete(kind);
    if (this.holds.size === 0) this.snapshot = null;
    else this.snapshot ??= live;
    return this.snapshot;
  }

  clear(): T[] | null {
    this.snapshot = this.holds.size ? [] : null;
    return this.snapshot;
  }

  key(line: T): number {
    let id = this.ids.get(line);
    if (id === undefined) {
      id = ++this.nextId;
      this.ids.set(line, id);
    }
    return id;
  }
}
