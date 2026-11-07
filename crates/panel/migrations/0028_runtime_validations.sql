CREATE TABLE runtime_validations (
    id UUID PRIMARY KEY,
    server_id BIGINT NOT NULL REFERENCES servers(id),
    module TEXT NOT NULL,
    scope TEXT NOT NULL,
    generation BIGINT NOT NULL CHECK(generation>0),
    operation TEXT NOT NULL CHECK(operation IN ('probe','barrier')),
    revision BIGINT NOT NULL CHECK(revision>0),
    config_hash TEXT NOT NULL,
    expires_at BIGINT NOT NULL,
    requested_at BIGINT NOT NULL,
    result JSONB,
    result_digest TEXT
);
CREATE INDEX runtime_validations_pending ON runtime_validations(server_id,requested_at) WHERE result IS NULL;
CREATE INDEX runtime_validations_attempts ON runtime_validations(server_id,module,scope,generation,operation,revision,requested_at DESC);
