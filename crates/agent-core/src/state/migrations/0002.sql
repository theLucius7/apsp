CREATE TABLE IF NOT EXISTS telemetry_outbox (
    id TEXT PRIMARY KEY,
    sampled_at INTEGER NOT NULL,
    payload TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS telemetry_time_idx ON telemetry_outbox(sampled_at);
