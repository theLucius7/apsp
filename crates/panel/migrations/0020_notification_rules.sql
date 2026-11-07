ALTER TABLE server_offline_events RENAME TO server_alert_events;
ALTER TABLE server_alert_events ALTER COLUMN last_seen DROP NOT NULL;
ALTER TABLE server_alert_events ADD COLUMN category TEXT NOT NULL DEFAULT 'offline'
    CHECK (category IN ('offline','resource','expiry','traffic'));
ALTER TABLE server_alert_events ADD COLUMN source_key TEXT NOT NULL DEFAULT 'offline';
ALTER TABLE server_alert_events ADD COLUMN details JSONB NOT NULL DEFAULT '{}'::jsonb;
ALTER TABLE server_alert_events ADD COLUMN message TEXT NOT NULL DEFAULT '';
DROP INDEX server_one_active_offline_event;
CREATE UNIQUE INDEX server_one_active_alert ON server_alert_events(server_id,source_key) WHERE resolved_at IS NULL;
CREATE INDEX server_alert_history ON server_alert_events(category,id DESC);

ALTER TABLE notification_outbox DROP CONSTRAINT notification_outbox_kind_check;
ALTER TABLE notification_outbox ADD CONSTRAINT notification_outbox_kind_check CHECK (kind IN ('offline','alert','recovery'));

CREATE TABLE alert_rules (
    id UUID PRIMARY KEY,
    spec JSONB NOT NULL,
    revision BIGINT NOT NULL DEFAULT 1
);
-- Reminder receipts outlive event-history cleanup, preventing a later replay in the same cycle.
CREATE TABLE alert_reminder_receipts (
    server_id BIGINT NOT NULL REFERENCES servers(id),
    source_key TEXT NOT NULL,
    retain_until BIGINT NOT NULL,
    PRIMARY KEY(server_id,source_key)
);
CREATE TABLE notification_test_limit (
    singleton BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK(singleton),
    sent_at BIGINT NOT NULL DEFAULT 0
);
INSERT INTO notification_test_limit(singleton) VALUES(TRUE);
