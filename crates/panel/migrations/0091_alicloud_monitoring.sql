ALTER TABLE alicloud_accounts ADD COLUMN traffic_history_identity TEXT;
ALTER TABLE alicloud_resources ADD COLUMN cdt_guard_next_at BIGINT NOT NULL DEFAULT 0;
ALTER TABLE alicloud_resources ADD COLUMN ipv6_addresses JSONB NOT NULL DEFAULT '[]';
ALTER TABLE alicloud_resources ADD COLUMN ipv6_checked_at BIGINT;
ALTER TABLE alicloud_resources ADD COLUMN ipv6_error TEXT;

CREATE TABLE alicloud_traffic_history (
    account_id UUID NOT NULL REFERENCES alicloud_accounts(id),
    bucket_seconds INTEGER NOT NULL CHECK(bucket_seconds IN (3600,86400)),
    bucket_start BIGINT NOT NULL,
    sampled_at BIGINT NOT NULL,
    mainland_bytes NUMERIC(20,0) NOT NULL CHECK(mainland_bytes >= 0),
    overseas_bytes NUMERIC(20,0) NOT NULL CHECK(overseas_bytes >= 0),
    PRIMARY KEY(account_id,bucket_seconds,bucket_start)
);
CREATE INDEX alicloud_traffic_history_retention ON alicloud_traffic_history(bucket_start);
