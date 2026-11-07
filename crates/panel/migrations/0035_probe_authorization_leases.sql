-- Authorization stays in the existing monitor object, with its exact target identity.
-- Existing targets, assignments, authorization records and historical samples are preserved.
ALTER TABLE network_probes ADD COLUMN revision BIGINT NOT NULL DEFAULT 1 CHECK (revision > 0);
ALTER TABLE servers ADD COLUMN probe_revision BIGINT NOT NULL DEFAULT 0 CHECK (probe_revision >= 0);
ALTER TABLE servers ADD COLUMN probe_fingerprint TEXT;

CREATE TABLE probe_leases (
    id UUID PRIMARY KEY,
    server_id BIGINT NOT NULL REFERENCES servers(id),
    session_hash TEXT NOT NULL,
    revision BIGINT NOT NULL CHECK (revision >= 0),
    issued_at BIGINT NOT NULL CHECK (issued_at > 0),
    expires_at BIGINT NOT NULL CHECK (expires_at > issued_at AND expires_at - issued_at <= 90),
    probe_digests JSONB NOT NULL CHECK (jsonb_typeof(probe_digests) = 'object')
);
CREATE INDEX probe_lease_server_time_idx ON probe_leases(server_id, issued_at DESC, id DESC);
