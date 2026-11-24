-- Published release versions accepted by the activity endpoint (#659).
CREATE TABLE IF NOT EXISTS release_versions (
  version    TEXT PRIMARY KEY,
  first_seen INTEGER NOT NULL,
  last_seen  INTEGER NOT NULL
);
