-- A delayed report may remain readable without replacing a newer observation.
ALTER TABLE diagnostic_jobs ADD COLUMN generation BIGSERIAL;

ALTER TABLE server_ip_quality
    ADD COLUMN source_job_created_at BIGINT,
    ADD COLUMN source_job_id UUID,
    ADD COLUMN source_section_revision BIGINT;

CREATE INDEX server_node_ip_quality_history_idx
    ON server_ip_quality(server_id,checked_at DESC,ip,provider)
    WHERE provider LIKE 'ipquality-node/%';

CREATE TABLE server_ip_quality_node_generations (
    server_id BIGINT NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
    ip_version TEXT NOT NULL CHECK (ip_version IN ('4','6')),
    job_created_at BIGINT NOT NULL,
    job_generation BIGINT NOT NULL,
    job_id UUID NOT NULL REFERENCES diagnostic_jobs(id) ON DELETE CASCADE,
    section_revision BIGINT NOT NULL CHECK (section_revision > 0),
    egress_ip TEXT,
    PRIMARY KEY(server_id,ip_version)
);
