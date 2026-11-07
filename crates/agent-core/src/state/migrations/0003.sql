CREATE TABLE IF NOT EXISTS command_journal (
    id TEXT PRIMARY KEY,
    spec TEXT NOT NULL,
    result TEXT,
    acknowledged INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS probe_outbox (
    id TEXT PRIMARY KEY,
    sampled_at INTEGER NOT NULL,
    result TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS probe_outbox_time_idx ON probe_outbox(sampled_at);
