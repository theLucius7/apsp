ALTER TABLE notification_outbox ADD COLUMN channel TEXT NOT NULL DEFAULT 'telegram'
    CHECK (channel IN ('telegram','webhook'));
ALTER TABLE notification_outbox ADD COLUMN last_attempt_at BIGINT;
ALTER TABLE notification_outbox ADD COLUMN delivered_at BIGINT;
ALTER TABLE notification_outbox DROP CONSTRAINT notification_outbox_event_id_kind_key;
ALTER TABLE notification_outbox ADD CONSTRAINT notification_outbox_event_kind_channel_key
    UNIQUE(event_id,kind,channel);
CREATE INDEX notification_channel_pending ON notification_outbox(channel,next_attempt_at,id)
    WHERE status='pending';

CREATE TABLE notification_channel_tests (
    channel TEXT PRIMARY KEY CHECK (channel IN ('telegram','webhook')),
    attempted_at BIGINT NOT NULL,
    success BOOLEAN,
    last_error TEXT
);
INSERT INTO notification_channel_tests(channel,attempted_at)
    SELECT 'telegram',sent_at FROM notification_test_limit WHERE sent_at>0;
