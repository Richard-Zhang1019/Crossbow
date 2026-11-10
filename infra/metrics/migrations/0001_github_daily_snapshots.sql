CREATE TABLE github_daily_snapshots (
    day TEXT PRIMARY KEY NOT NULL,
    collected_at TEXT NOT NULL,
    stars INTEGER NOT NULL CHECK (stars >= 0),
    assets_json TEXT NOT NULL CHECK (json_valid(assets_json) AND json_type(assets_json) = 'array')
);
