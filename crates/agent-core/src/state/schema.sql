CREATE TABLE kv (
    key TEXT PRIMARY KEY NOT NULL,
    value TEXT NOT NULL
);

CREATE TABLE intents (
    op_id TEXT PRIMARY KEY NOT NULL,
    module TEXT NOT NULL,
    payload TEXT NOT NULL,
    completed INTEGER NOT NULL DEFAULT 0 CHECK (completed IN (0, 1))
);
CREATE INDEX intents_pending_idx ON intents(completed);

CREATE TABLE usage_baselines (
    module TEXT NOT NULL,
    stat_name TEXT NOT NULL,
    epoch TEXT NOT NULL,
    uplink TEXT NOT NULL,
    downlink TEXT NOT NULL,
    observed_at INTEGER NOT NULL,
    PRIMARY KEY (module, stat_name)
);

CREATE TABLE usage_outbox (
    epoch TEXT NOT NULL,
    seq TEXT NOT NULL,
    batch TEXT NOT NULL,
    acknowledged INTEGER NOT NULL DEFAULT 0 CHECK (acknowledged IN (0, 1)),
    PRIMARY KEY (epoch, seq),
    UNIQUE (seq)
);
CREATE INDEX usage_outbox_pending_idx ON usage_outbox(acknowledged);

CREATE TABLE telemetry_outbox (
    id TEXT PRIMARY KEY,
    sampled_at INTEGER NOT NULL,
    payload TEXT NOT NULL
);
CREATE INDEX telemetry_time_idx ON telemetry_outbox(sampled_at);

CREATE TABLE command_journal (
    id TEXT PRIMARY KEY,
    spec TEXT NOT NULL,
    result TEXT,
    acknowledged INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE probe_outbox (
    id TEXT PRIMARY KEY,
    sampled_at INTEGER NOT NULL,
    result TEXT NOT NULL
);
CREATE INDEX probe_outbox_time_idx ON probe_outbox(sampled_at);

CREATE INDEX usage_outbox_pending_order_idx
    ON usage_outbox(length(seq), seq)
    WHERE acknowledged = 0 AND octet_length(batch) <= 1048447;
CREATE INDEX usage_outbox_pending_bytes_idx
    ON usage_outbox(octet_length(batch)) WHERE acknowledged = 0;

CREATE TABLE command_lifecycle (
    id TEXT PRIMARY KEY REFERENCES command_journal(id) ON DELETE CASCADE,
    claim_id TEXT NOT NULL,
    process TEXT,
    started TEXT,
    started_acknowledged INTEGER NOT NULL DEFAULT 0,
    cancel_requested INTEGER NOT NULL DEFAULT 0,
    recovering INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE runtime_control (
    request_id TEXT PRIMARY KEY NOT NULL,
    digest TEXT NOT NULL,
    kind TEXT NOT NULL,
    module TEXT NOT NULL,
    request TEXT NOT NULL,
    result TEXT,
    acknowledged INTEGER NOT NULL DEFAULT 0 CHECK (acknowledged IN (0, 1))
);
CREATE INDEX runtime_control_pending ON runtime_control(acknowledged, result);
CREATE TABLE runtime_revision_floors (
    module TEXT PRIMARY KEY NOT NULL,
    revision TEXT NOT NULL
);

CREATE TABLE schema_identity (singleton INTEGER PRIMARY KEY CHECK(singleton=1), source_hash TEXT NOT NULL, catalog_hash TEXT NOT NULL);
