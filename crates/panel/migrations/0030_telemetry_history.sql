ALTER TABLE servers ADD COLUMN telemetry_settings JSONB NOT NULL DEFAULT '{"persist_interval_secs":60}';

CREATE TABLE telemetry_policy (
    singleton BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (singleton),
    history_retention_days INTEGER NOT NULL DEFAULT 30 CHECK (history_retention_days BETWEEN 1 AND 3650)
);
INSERT INTO telemetry_policy(singleton) VALUES(TRUE);

-- Retain identity independently of raw samples for the full accepted replay window.
CREATE TABLE telemetry_receipts (
    server_id BIGINT NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
    id UUID NOT NULL,
    sampled_at BIGINT NOT NULL,
    digest TEXT NOT NULL,
    PRIMARY KEY(server_id,id)
);
CREATE INDEX telemetry_receipts_expiry_idx ON telemetry_receipts(sampled_at);
CREATE INDEX telemetry_samples_expiry_idx ON telemetry_samples(sampled_at);
INSERT INTO telemetry_receipts SELECT server_id,id,sampled_at,digest FROM telemetry_samples;

CREATE TABLE telemetry_history (
    server_id BIGINT NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
    resolution_secs INTEGER NOT NULL CHECK (resolution_secs IN (60,300,3600)),
    bucket_at BIGINT NOT NULL,
    summary JSONB NOT NULL,
    PRIMARY KEY(server_id,resolution_secs,bucket_at)
);
CREATE INDEX telemetry_history_age_idx ON telemetry_history(bucket_at,server_id);

-- Old minute rows contain only their final observation, never invented counts.
CREATE TABLE telemetry_history_initialized (
    server_id BIGINT PRIMARY KEY REFERENCES servers(id) ON DELETE CASCADE
);
