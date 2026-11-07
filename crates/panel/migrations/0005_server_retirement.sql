CREATE TABLE server_retirements (
    server_id BIGINT PRIMARY KEY REFERENCES servers(id),
    request_id UUID NOT NULL UNIQUE,
    status TEXT NOT NULL CHECK (status IN ('pending', 'failed', 'confirmed', 'offline_unconfirmed')),
    error TEXT,
    requested_at BIGINT NOT NULL,
    completed_at BIGINT
);
