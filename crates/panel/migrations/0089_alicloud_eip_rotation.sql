-- Replacing an ECS instance's EIP until one is reachable from mainland China
-- (ADR 0116). A row is the durable intent: every step re-reads the cloud before
-- acting, so a restarted panel continues from the saved phase instead of
-- repeating a request. Steps call the cloud outside any transaction and store
-- their result only if `version` is still the one they read.
CREATE TABLE alicloud_eip_rotations (
    id UUID PRIMARY KEY,
    resource_id UUID NOT NULL REFERENCES alicloud_resources(id),
    requested_by BIGINT NOT NULL REFERENCES admins(id),
    before_state JSONB NOT NULL,
    probe_port INTEGER NOT NULL CHECK(probe_port BETWEEN 1 AND 65535),
    max_attempts INTEGER NOT NULL CHECK(max_attempts BETWEEN 1 AND 12),
    snapshot_digest TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('preview','cancelled','running','unknown','succeeded','restored','failed','reconciled')),
    phase TEXT NOT NULL DEFAULT 'allocate',
    phase_started_at BIGINT NOT NULL DEFAULT 0,
    attempt INTEGER NOT NULL DEFAULT 1,
    bound_id TEXT NOT NULL,
    candidate JSONB,
    measurement_id TEXT,
    drop_ids TEXT[] NOT NULL DEFAULT '{}',
    after_drop TEXT,
    attempts JSONB NOT NULL DEFAULT '[]'::JSONB,
    new_ip TEXT,
    error_code TEXT,
    created_at BIGINT NOT NULL,
    expires_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
    next_check_at BIGINT NOT NULL DEFAULT 0,
    reconciliation JSONB,
    version BIGINT NOT NULL DEFAULT 0
);
CREATE UNIQUE INDEX alicloud_eip_rotation_active ON alicloud_eip_rotations(resource_id)
    WHERE status IN ('running','unknown');
CREATE INDEX alicloud_eip_rotation_due ON alicloud_eip_rotations(next_check_at)
    WHERE status='running';
CREATE INDEX alicloud_eip_rotation_history ON alicloud_eip_rotations(resource_id,created_at DESC);
