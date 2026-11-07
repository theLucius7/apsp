CREATE TABLE runtime_operations (
    id UUID PRIMARY KEY,
    server_id BIGINT NOT NULL REFERENCES servers(id),
    module TEXT NOT NULL,
    requested_at BIGINT NOT NULL,
    dispatched_at BIGINT,
    spec JSONB NOT NULL,
    result JSONB,
    CHECK (char_length(module) BETWEEN 1 AND 64)
);
CREATE UNIQUE INDEX runtime_operations_one_active ON runtime_operations(server_id, module) WHERE result IS NULL;
CREATE INDEX runtime_operations_history ON runtime_operations(server_id, requested_at DESC);
