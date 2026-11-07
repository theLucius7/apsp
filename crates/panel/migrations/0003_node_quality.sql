ALTER TABLE servers ADD COLUMN capabilities JSONB NOT NULL DEFAULT '[]'::jsonb;
ALTER TABLE servers ADD COLUMN quality_refresh_started BIGINT;

CREATE TABLE diagnostic_jobs (
    id UUID PRIMARY KEY,
    server_id BIGINT NOT NULL REFERENCES servers(id),
    job JSONB NOT NULL,
    status TEXT NOT NULL DEFAULT 'queued' CHECK (status IN ('queued', 'running', 'succeeded', 'failed')),
    report JSONB,
    error TEXT,
    agent_completed BOOLEAN NOT NULL DEFAULT FALSE,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
    expires_at BIGINT NOT NULL
);
CREATE UNIQUE INDEX diagnostic_active_server_idx ON diagnostic_jobs(server_id) WHERE status IN ('queued', 'running');
CREATE INDEX diagnostic_history_idx ON diagnostic_jobs(server_id, created_at DESC);
CREATE INDEX diagnostic_expiry_idx ON diagnostic_jobs(expires_at) WHERE status IN ('queued', 'running');

CREATE TABLE server_ip_quality (
    server_id BIGINT NOT NULL REFERENCES servers(id),
    ip TEXT NOT NULL,
    payload JSONB NOT NULL,
    checked_at BIGINT NOT NULL,
    PRIMARY KEY (server_id, ip)
);
