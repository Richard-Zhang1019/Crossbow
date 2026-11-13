CREATE TABLE github_snapshot_cooldown (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  retry_at INTEGER NOT NULL
);
