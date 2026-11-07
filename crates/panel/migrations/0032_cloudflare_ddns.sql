ALTER TABLE servers ADD COLUMN static_info_received_at BIGINT;

CREATE TABLE ddns_rules (
    id UUID PRIMARY KEY,
    server_id BIGINT NOT NULL REFERENCES servers(id),
    config JSONB NOT NULL,
    api_token TEXT NOT NULL,
    revision BIGINT NOT NULL DEFAULT 1,
    record_id TEXT,
    last_ip TEXT,
    last_success_at BIGINT,
    attempted_at BIGINT,
    next_run_at BIGINT NOT NULL DEFAULT 0,
    failures INTEGER NOT NULL DEFAULT 0,
    status TEXT NOT NULL DEFAULT 'pending',
    error_code TEXT,
    lease_id UUID,
    lease_until BIGINT NOT NULL DEFAULT 0,
    CHECK ((config->>'server_id')::bigint = server_id)
);
CREATE UNIQUE INDEX ddns_record_identity ON ddns_rules
    ((config->>'zone_id'), (config->>'record_name'), (config->>'record_type'));
CREATE INDEX ddns_due ON ddns_rules(next_run_at);
