-- These identities do not change the historical deployment or subscription keys.
CREATE TABLE runtime_control_devices (
    server_id BIGINT PRIMARY KEY REFERENCES servers(id),
    checkpoint_required BOOLEAN NOT NULL DEFAULT FALSE
);

CREATE TABLE runtime_deployment_bindings (
    deployment_id UUID PRIMARY KEY,
    server_id BIGINT NOT NULL,
    module TEXT NOT NULL,
    rev BIGINT NOT NULL CHECK (rev > 0),
    bundle_sha256 TEXT NOT NULL CHECK (bundle_sha256 ~ '^[0-9a-f]{64}$'),
    binding_digest TEXT NOT NULL CHECK (binding_digest ~ '^[0-9a-f]{64}$'),
    UNIQUE (server_id, module, rev),
    FOREIGN KEY (server_id, module, rev) REFERENCES deployments(server_id, module, rev)
);

CREATE TABLE runtime_control_requests (
    request_id UUID PRIMARY KEY,
    server_id BIGINT NOT NULL REFERENCES servers(id),
    module TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('checkpoint', 'barrier')),
    request_digest TEXT NOT NULL CHECK (request_digest ~ '^[0-9a-f]{64}$'),
    request_json JSONB NOT NULL,
    created_at BIGINT NOT NULL,
    expires_at BIGINT NOT NULL CHECK (expires_at > created_at AND expires_at <= created_at + 600),
    state TEXT NOT NULL DEFAULT 'pending' CHECK (state IN ('pending', 'received', 'expired'))
);

CREATE TABLE runtime_apply_facts (
    server_id BIGINT NOT NULL REFERENCES servers(id),
    op_id UUID NOT NULL,
    module TEXT NOT NULL,
    rev BIGINT NOT NULL,
    result_json JSONB NOT NULL,
    received_at BIGINT NOT NULL,
    PRIMARY KEY (server_id, op_id),
    FOREIGN KEY (server_id, module, rev) REFERENCES deployments(server_id, module, rev)
);
CREATE UNIQUE INDEX runtime_control_pending_idx
    ON runtime_control_requests(server_id, module, kind) WHERE state = 'pending';
CREATE INDEX runtime_control_replay_idx
    ON runtime_control_requests(server_id, created_at) WHERE state = 'pending';

CREATE TABLE runtime_control_receipts (
    request_id UUID PRIMARY KEY REFERENCES runtime_control_requests(request_id),
    result_json JSONB NOT NULL,
    result_sha256 TEXT NOT NULL CHECK (result_sha256 ~ '^[0-9a-f]{64}$'),
    received_at BIGINT NOT NULL,
    outcome TEXT NOT NULL CHECK (outcome IN ('verified', 'late', 'superseded', 'mismatch', 'failed'))
);

CREATE TABLE runtime_module_checkpoints (
    server_id BIGINT NOT NULL REFERENCES servers(id),
    module TEXT NOT NULL,
    checkpoint_json JSONB NOT NULL,
    checkpoint_request_id UUID NOT NULL REFERENCES runtime_control_receipts(request_id),
    verified_at BIGINT NOT NULL,
    minimum_revision BIGINT NOT NULL DEFAULT 0 CHECK (minimum_revision >= 0),
    barrier_request_id UUID REFERENCES runtime_control_receipts(request_id),
    PRIMARY KEY (server_id, module)
);
