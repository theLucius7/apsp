CREATE TABLE alicloud_accounts (
    id UUID PRIMARY KEY,
    name TEXT NOT NULL,
    site TEXT NOT NULL DEFAULT 'china' CHECK(site IN ('china','international')),
    access_key_id TEXT NOT NULL,
    access_key_secret TEXT NOT NULL,
    enabled BOOLEAN NOT NULL DEFAULT true,
    auto_enabled BOOLEAN NOT NULL DEFAULT false,
    limit_gb BIGINT NOT NULL DEFAULT 100 CHECK(limit_gb BETWEEN 1 AND 1000000000),
    revision BIGINT NOT NULL DEFAULT 1,
    bill JSONB,
    traffic JSONB,
    traffic_error TEXT,
    error_code TEXT,
    next_run_at BIGINT NOT NULL DEFAULT 0,
    last_attempt_at BIGINT NOT NULL DEFAULT 0,
    archived BOOLEAN NOT NULL DEFAULT false
);
CREATE TABLE alicloud_resources (
    id UUID PRIMARY KEY,
    account_id UUID NOT NULL REFERENCES alicloud_accounts(id),
    name TEXT NOT NULL,
    kind TEXT NOT NULL CHECK(kind IN ('ecs','eip')),
    region TEXT NOT NULL,
    cloud_id TEXT NOT NULL,
    auto_enabled BOOLEAN NOT NULL DEFAULT false,
    cap_mbps BIGINT NOT NULL DEFAULT 1 CHECK(cap_mbps BETWEEN 1 AND 100),
    revision BIGINT NOT NULL DEFAULT 1,
    snapshot JSONB,
    checked_at BIGINT,
    last_attempt_at BIGINT NOT NULL DEFAULT 0,
    error_code TEXT,
    archived BOOLEAN NOT NULL DEFAULT false
);
CREATE UNIQUE INDEX alicloud_resource_identity ON alicloud_resources(kind,region,cloud_id) WHERE NOT archived;
CREATE TABLE alicloud_operations (
    id UUID PRIMARY KEY,
    resource_id UUID NOT NULL REFERENCES alicloud_resources(id),
    account_revision BIGINT NOT NULL,
    resource_revision BIGINT NOT NULL,
    before_state JSONB NOT NULL,
    target JSONB NOT NULL,
    source TEXT NOT NULL CHECK(source IN ('manual','automatic')),
    billing_cycle TEXT,
    status TEXT NOT NULL CHECK(status IN ('preview','queued','running','uncertain','succeeded','failed','cancelled','dismissed')),
    created_at BIGINT NOT NULL,
    expires_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
    next_check_at BIGINT NOT NULL DEFAULT 0,
    error_code TEXT,
    request_id TEXT
);
CREATE UNIQUE INDEX alicloud_operation_active ON alicloud_operations(resource_id) WHERE status IN ('queued','running','uncertain');
CREATE UNIQUE INDEX alicloud_automatic_cycle ON alicloud_operations(resource_id,billing_cycle,account_revision,resource_revision) WHERE source='automatic';
CREATE INDEX alicloud_operation_history ON alicloud_operations(created_at DESC);
