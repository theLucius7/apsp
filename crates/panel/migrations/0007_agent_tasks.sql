CREATE TABLE remote_commands (
    id UUID PRIMARY KEY,
    server_id BIGINT NOT NULL REFERENCES servers(id),
    requested_at BIGINT NOT NULL,
    spec JSONB NOT NULL,
    result JSONB,
    result_digest TEXT
);
CREATE INDEX command_server_idx ON remote_commands(server_id, requested_at);
CREATE TABLE network_probes (
    id UUID PRIMARY KEY,
    server_id BIGINT NOT NULL REFERENCES servers(id),
    spec JSONB NOT NULL
);
CREATE INDEX probe_server_idx ON network_probes(server_id);
CREATE TABLE probe_results (
    id UUID PRIMARY KEY,
    server_id BIGINT NOT NULL REFERENCES servers(id),
    probe_id UUID NOT NULL,
    sampled_at BIGINT NOT NULL,
    result JSONB NOT NULL,
    digest TEXT NOT NULL
);
CREATE INDEX probe_result_time_idx ON probe_results(server_id, sampled_at);
