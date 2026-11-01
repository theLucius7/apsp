-- China reachability monitor. Disabled by default: enabling it sends node IP
-- addresses and ports to a third-party measurement service.
CREATE TABLE singbox_reachability_settings (
    singleton BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (singleton),
    enabled BOOLEAN NOT NULL DEFAULT FALSE,
    interval_minutes INTEGER NOT NULL DEFAULT 30 CHECK (interval_minutes BETWEEN 15 AND 1440),
    credential_id UUID REFERENCES credential_entries(id),
    revision BIGINT NOT NULL DEFAULT 1 CHECK (revision > 0),
    next_run_at BIGINT NOT NULL DEFAULT 0,
    run_requested_at BIGINT,
    lease_until BIGINT,
    last_run JSONB NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(last_run) = 'object')
);
INSERT INTO singbox_reachability_settings(singleton) VALUES (TRUE);

-- One row per deduplicated public endpoint. Nodes are matched by host and port
-- at read time, so the row outlives a node being recreated on the same endpoint.
CREATE TABLE singbox_reachability_targets (
    id BIGSERIAL PRIMARY KEY,
    host TEXT NOT NULL CHECK (host <> '' AND length(host) <= 253),
    port INTEGER NOT NULL CHECK (port BETWEEN 1 AND 65535),
    state TEXT NOT NULL DEFAULT 'unknown' CHECK (state IN ('reachable','partial','service_down','port_blocked','ip_blocked','unconfirmed','unknown','not_applicable')),
    pending_state TEXT CHECK (pending_state IN ('port_blocked','ip_blocked','unconfirmed')),
    failing_streak INTEGER NOT NULL DEFAULT 0 CHECK (failing_streak >= 0),
    observed_state TEXT CHECK (observed_state IN ('reachable','partial','service_down','port_blocked','ip_blocked','unconfirmed','unknown','not_applicable')),
    resolved_ip TEXT,
    detail JSONB NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(detail) = 'object'),
    checked_at BIGINT,
    state_changed_at BIGINT,
    deferred_at BIGINT,
    seen_at BIGINT NOT NULL,
    UNIQUE (host, port)
);
CREATE INDEX singbox_reachability_targets_seen ON singbox_reachability_targets(seen_at);

CREATE TABLE singbox_reachability_history (
    id BIGSERIAL PRIMARY KEY,
    target_id BIGINT NOT NULL REFERENCES singbox_reachability_targets(id) ON DELETE CASCADE,
    checked_at BIGINT NOT NULL,
    state TEXT NOT NULL,
    observed_state TEXT NOT NULL,
    detail JSONB NOT NULL
);
CREATE INDEX singbox_reachability_history_target ON singbox_reachability_history(target_id, id DESC);
CREATE INDEX singbox_reachability_history_age ON singbox_reachability_history(checked_at);

-- Transitions into and out of a blocked state, delivered like expense reminders.
CREATE TABLE singbox_reachability_events (
    id UUID PRIMARY KEY,
    target_id BIGINT REFERENCES singbox_reachability_targets(id) ON DELETE SET NULL,
    server_id BIGINT REFERENCES servers(id),
    kind TEXT NOT NULL CHECK (kind IN ('blocked','cleared')),
    from_state TEXT NOT NULL,
    to_state TEXT NOT NULL,
    message TEXT NOT NULL,
    created_at BIGINT NOT NULL
);
CREATE INDEX singbox_reachability_events_created ON singbox_reachability_events(created_at DESC, id);

CREATE TABLE singbox_reachability_deliveries (
    id BIGSERIAL PRIMARY KEY,
    event_id UUID NOT NULL REFERENCES singbox_reachability_events(id) ON DELETE CASCADE,
    channel TEXT NOT NULL CHECK (channel IN ('telegram','webhook')),
    payload TEXT NOT NULL,
    state TEXT NOT NULL DEFAULT 'pending' CHECK (state IN ('pending','sending','sent','failed','cancelled')),
    attempts INTEGER NOT NULL DEFAULT 0,
    next_attempt_at BIGINT NOT NULL,
    lease_until BIGINT,
    last_error TEXT,
    UNIQUE (event_id, channel)
);
CREATE INDEX singbox_reachability_deliveries_pending ON singbox_reachability_deliveries(next_attempt_at, id)
    WHERE state IN ('pending','sending','failed');
