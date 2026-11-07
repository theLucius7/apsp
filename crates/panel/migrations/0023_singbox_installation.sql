-- Panel preparation failures are distinct from device ApplyResult evidence.
CREATE TABLE singbox_installation (
    server_id BIGINT PRIMARY KEY REFERENCES servers(id) ON DELETE CASCADE,
    target_rev BIGINT NOT NULL CHECK (target_rev > 0),
    error TEXT CHECK (error IS NULL OR char_length(error) BETWEEN 1 AND 2048),
    checked_at BIGINT NOT NULL
);
