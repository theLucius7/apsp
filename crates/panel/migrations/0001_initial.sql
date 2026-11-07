CREATE TABLE admins (
    id BIGINT PRIMARY KEY CHECK (id = 1),
    password_hash TEXT NOT NULL
);

CREATE TABLE servers (
    id BIGSERIAL PRIMARY KEY,
    name TEXT NOT NULL,
    device_public_key TEXT,
    static_info JSONB NOT NULL DEFAULT '{}'::jsonb,
    last_seen BIGINT,
    latest_metrics JSONB NOT NULL DEFAULT '{}'::jsonb,
    manifest_rev BIGINT NOT NULL DEFAULT 0 CHECK (manifest_rev >= 0),
    dirty_at BIGINT,
    deleted_at BIGINT
);

CREATE TABLE sessions (
    token_hash TEXT PRIMARY KEY,
    admin_id BIGINT REFERENCES admins(id),
    server_id BIGINT REFERENCES servers(id),
    expires_at BIGINT NOT NULL,
    CHECK ((admin_id IS NOT NULL) <> (server_id IS NOT NULL))
);
CREATE INDEX sessions_expiry_idx ON sessions(expires_at);
CREATE INDEX sessions_server_idx ON sessions(server_id);

CREATE TABLE enrollment_tokens (
    token_hash TEXT PRIMARY KEY,
    server_id BIGINT NOT NULL REFERENCES servers(id),
    expires_at BIGINT NOT NULL,
    consumed_at BIGINT
);
CREATE INDEX enrollment_server_idx ON enrollment_tokens(server_id);

CREATE TABLE nodes (
    id BIGSERIAL PRIMARY KEY,
    name TEXT NOT NULL,
    server_id BIGINT NOT NULL REFERENCES servers(id),
    protocol TEXT NOT NULL DEFAULT 'vless-reality' CHECK (protocol = 'vless-reality'),
    port INTEGER NOT NULL CHECK (port BETWEEN 1 AND 65535),
    public_host TEXT NOT NULL,
    sni TEXT NOT NULL,
    private_key TEXT NOT NULL,
    public_key TEXT NOT NULL,
    short_id TEXT NOT NULL,
    deleted_at BIGINT
);
CREATE UNIQUE INDEX nodes_active_port_idx ON nodes(server_id, port) WHERE deleted_at IS NULL;

CREATE TABLE users (
    id BIGSERIAL PRIMARY KEY,
    name TEXT NOT NULL,
    subscription_token TEXT NOT NULL UNIQUE,
    deleted_at BIGINT
);

CREATE TABLE accesses (
    user_id BIGINT NOT NULL REFERENCES users(id),
    node_id BIGINT NOT NULL REFERENCES nodes(id),
    uuid UUID NOT NULL UNIQUE,
    stat_name TEXT NOT NULL UNIQUE,
    PRIMARY KEY (user_id, node_id)
);
CREATE INDEX accesses_node_idx ON accesses(node_id);

CREATE TABLE deployments (
    server_id BIGINT NOT NULL REFERENCES servers(id),
    module TEXT NOT NULL,
    rev BIGINT NOT NULL CHECK (rev > 0),
    bundle TEXT NOT NULL,
    bundle_sha256 TEXT NOT NULL,
    source_json JSONB NOT NULL DEFAULT '[]'::jsonb,
    created_at BIGINT NOT NULL,
    PRIMARY KEY (server_id, module, rev)
);

CREATE TABLE server_module_status (
    server_id BIGINT NOT NULL REFERENCES servers(id),
    module TEXT NOT NULL,
    target_rev BIGINT NOT NULL DEFAULT 0 CHECK (target_rev >= 0),
    applied_rev BIGINT NOT NULL DEFAULT 0 CHECK (applied_rev >= 0),
    last_result_rev BIGINT NOT NULL DEFAULT 0 CHECK (last_result_rev >= 0),
    healthy BOOLEAN NOT NULL DEFAULT FALSE,
    last_error TEXT,
    updated_at BIGINT NOT NULL DEFAULT 0,
    PRIMARY KEY (server_id, module)
);

CREATE TABLE usage_batches (
    server_id BIGINT NOT NULL REFERENCES servers(id),
    epoch UUID NOT NULL,
    seq BIGINT NOT NULL CHECK (seq >= 0),
    payload_hash TEXT NOT NULL,
    PRIMARY KEY (server_id, epoch, seq)
);

CREATE TABLE usage_records (
    server_id BIGINT NOT NULL REFERENCES servers(id),
    epoch UUID NOT NULL,
    seq BIGINT NOT NULL CHECK (seq >= 0),
    stat_name TEXT NOT NULL,
    user_id BIGINT NOT NULL REFERENCES users(id),
    node_id BIGINT NOT NULL REFERENCES nodes(id),
    uplink BIGINT NOT NULL CHECK (uplink >= 0),
    downlink BIGINT NOT NULL CHECK (downlink >= 0),
    period_start BIGINT NOT NULL,
    period_end BIGINT NOT NULL CHECK (period_end >= period_start),
    UNIQUE (server_id, epoch, seq, stat_name),
    FOREIGN KEY (server_id, epoch, seq) REFERENCES usage_batches(server_id, epoch, seq)
);
CREATE INDEX usage_records_user_idx ON usage_records(user_id);
CREATE INDEX usage_records_node_idx ON usage_records(node_id);

CREATE TABLE metrics_minutely (
    server_id BIGINT NOT NULL REFERENCES servers(id),
    bucket BIGINT NOT NULL,
    metrics JSONB NOT NULL,
    PRIMARY KEY (server_id, bucket)
);
CREATE INDEX metrics_retention_idx ON metrics_minutely(bucket);
