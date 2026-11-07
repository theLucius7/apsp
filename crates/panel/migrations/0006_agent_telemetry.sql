ALTER TABLE servers ADD COLUMN agent_settings JSONB NOT NULL DEFAULT '{"sample_interval_secs":1,"upload_interval_secs":3,"auto_update":false,"discover_public_ips":true}'::jsonb;
ALTER TABLE servers ADD COLUMN metrics_sampled_at BIGINT NOT NULL DEFAULT 0;
ALTER TABLE metrics_minutely ADD COLUMN sampled_at BIGINT NOT NULL DEFAULT 0;
CREATE TABLE telemetry_samples (
    server_id BIGINT NOT NULL REFERENCES servers(id),
    id UUID NOT NULL,
    sampled_at BIGINT NOT NULL,
    digest TEXT NOT NULL,
    metrics JSONB NOT NULL,
    PRIMARY KEY(server_id,id)
);
CREATE INDEX telemetry_samples_time_idx ON telemetry_samples(server_id,sampled_at);
