CREATE TABLE panel_settings (
    singleton BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (singleton),
    settings JSONB NOT NULL DEFAULT '{}'::jsonb
);
INSERT INTO panel_settings(singleton) VALUES(TRUE);

CREATE TABLE server_offline_events (
    id BIGSERIAL PRIMARY KEY,
    server_id BIGINT NOT NULL REFERENCES servers(id),
    server_name TEXT NOT NULL,
    last_seen BIGINT NOT NULL,
    opened_at BIGINT NOT NULL,
    resolved_at BIGINT,
    resolution TEXT
);
CREATE UNIQUE INDEX server_one_active_offline_event ON server_offline_events(server_id) WHERE resolved_at IS NULL;
CREATE TABLE notification_outbox (
    id BIGSERIAL PRIMARY KEY,
    event_id BIGINT NOT NULL REFERENCES server_offline_events(id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK (kind IN ('offline','recovery')),
    message TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending','sent','failed','cancelled')),
    attempts INTEGER NOT NULL DEFAULT 0,
    next_attempt_at BIGINT NOT NULL,
    last_error TEXT,
    UNIQUE(event_id,kind)
);
CREATE INDEX notification_pending ON notification_outbox(next_attempt_at,id) WHERE status='pending';

CREATE TABLE server_traffic_corrections (
    id BIGSERIAL PRIMARY KEY,
    server_id BIGINT NOT NULL REFERENCES servers(id),
    cycle_start BIGINT NOT NULL,
    reset_day INTEGER NOT NULL,
    network_interface TEXT NOT NULL,
    uploaded_offset NUMERIC(39,0) NOT NULL,
    downloaded_offset NUMERIC(39,0) NOT NULL,
    reason TEXT NOT NULL,
    created_at BIGINT NOT NULL,
    invalidated_at BIGINT
);
CREATE INDEX server_traffic_correction_lookup ON server_traffic_corrections(server_id,cycle_start,reset_day,network_interface,id DESC);
