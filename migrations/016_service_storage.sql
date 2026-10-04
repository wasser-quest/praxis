CREATE TABLE IF NOT EXISTS service_storage (
    owner TEXT NOT NULL,
    user_id TEXT NOT NULL,
    key TEXT NOT NULL,
    value TEXT NOT NULL,
    PRIMARY KEY (owner, user_id, key)
);
