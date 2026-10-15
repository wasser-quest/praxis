-- Append-only change log per context row (see src/db/context_events.rs).
CREATE TABLE IF NOT EXISTS context_events (
    user_key TEXT NOT NULL,
    seq INTEGER NOT NULL,
    kind TEXT NOT NULL,
    source TEXT NOT NULL,
    payload TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    PRIMARY KEY (user_key, seq)
);
