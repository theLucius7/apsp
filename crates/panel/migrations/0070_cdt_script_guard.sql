ALTER TABLE alicloud_resources
    ADD COLUMN cdt_guard_state jsonb,
    ADD COLUMN cdt_guard_error text;

ALTER TABLE alicloud_power_jobs DROP CONSTRAINT alicloud_power_jobs_source_check;
ALTER TABLE alicloud_power_jobs ADD CONSTRAINT alicloud_power_jobs_source_check
    CHECK (source IN ('manual', 'threshold', 'schedule', 'keepalive', 'cdt_guard'));
