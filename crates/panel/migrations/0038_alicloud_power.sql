ALTER TABLE alicloud_accounts ADD COLUMN balance JSONB;
ALTER TABLE alicloud_accounts ADD COLUMN balance_error TEXT;
ALTER TABLE alicloud_accounts ADD COLUMN balance_next_at BIGINT NOT NULL DEFAULT 0;
ALTER TABLE alicloud_resources ADD COLUMN instance_bill JSONB;
ALTER TABLE alicloud_resources ADD COLUMN bill_error TEXT;
ALTER TABLE alicloud_resources ADD COLUMN bill_next_at BIGINT NOT NULL DEFAULT 0;
ALTER TABLE alicloud_resources ADD COLUMN power_policy JSONB NOT NULL DEFAULT '{"enabled":false,"stop_mode":"KeepCharging","threshold_action":"off","limit_gb":100,"threshold_percent":95,"schedule_enabled":false,"start_time":"08:00","stop_time":"23:00","utc_offset_minutes":480,"keepalive":false}';
ALTER TABLE alicloud_resources ADD COLUMN power_state JSONB;
ALTER TABLE alicloud_resources ADD COLUMN power_checked_at BIGINT;
ALTER TABLE alicloud_resources ADD COLUMN power_error TEXT;
ALTER TABLE alicloud_resources ADD COLUMN next_power_at BIGINT NOT NULL DEFAULT 0;
ALTER TABLE alicloud_resources ADD COLUMN manual_hold BOOLEAN NOT NULL DEFAULT false;
ALTER TABLE alicloud_resources ADD COLUMN threshold_hold BOOLEAN NOT NULL DEFAULT false;

CREATE TABLE alicloud_power_jobs (
    id UUID PRIMARY KEY,
    resource_id UUID NOT NULL REFERENCES alicloud_resources(id),
    account_revision BIGINT NOT NULL,
    resource_revision BIGINT NOT NULL,
    action TEXT NOT NULL CHECK(action IN ('start','stop')),
    stop_mode TEXT NOT NULL CHECK(stop_mode IN ('KeepCharging','StopCharging')),
    source TEXT NOT NULL CHECK(source IN ('manual','threshold','schedule','keepalive')),
    dedup_key TEXT UNIQUE,
    before_state JSONB NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('preview','queued','running','uncertain','succeeded','failed','cancelled','dismissed')),
    created_at BIGINT NOT NULL,
    expires_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
    next_check_at BIGINT NOT NULL DEFAULT 0,
    request_id TEXT,
    error_code TEXT
);
CREATE UNIQUE INDEX alicloud_power_active ON alicloud_power_jobs(resource_id) WHERE status IN ('queued','running','uncertain');
CREATE INDEX alicloud_power_pending ON alicloud_power_jobs(next_check_at,created_at) WHERE status IN ('queued','running','uncertain');

CREATE TABLE alicloud_events (
    id BIGSERIAL PRIMARY KEY,
    resource_id UUID NOT NULL REFERENCES alicloud_resources(id),
    dedup_key TEXT NOT NULL UNIQUE,
    title TEXT NOT NULL,
    message TEXT NOT NULL,
    created_at BIGINT NOT NULL
);
CREATE TABLE alicloud_deliveries (
    id BIGSERIAL PRIMARY KEY,
    event_id BIGINT NOT NULL REFERENCES alicloud_events(id) ON DELETE CASCADE,
    channel TEXT NOT NULL CHECK(channel IN ('telegram','webhook')),
    payload TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending' CHECK(status IN ('pending','sent','failed','cancelled')),
    attempts INT NOT NULL DEFAULT 0,
    next_attempt_at BIGINT NOT NULL,
    last_error TEXT,
    delivered_at BIGINT,
    UNIQUE(event_id,channel)
);
