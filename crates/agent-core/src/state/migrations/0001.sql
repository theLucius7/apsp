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
