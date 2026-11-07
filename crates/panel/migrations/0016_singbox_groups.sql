-- Plugin-owned authorization and immutable package assignments.
ALTER TABLE accesses ADD COLUMN direct_grant BOOLEAN NOT NULL DEFAULT TRUE;

CREATE TABLE singbox_policy_groups (
    id BIGSERIAL PRIMARY KEY,
    name TEXT NOT NULL
);
CREATE TABLE singbox_chains (
    id BIGSERIAL PRIMARY KEY,
    name TEXT NOT NULL,
    entry_node_id BIGINT NOT NULL UNIQUE REFERENCES nodes(id),
    exit_node_id BIGINT NOT NULL REFERENCES nodes(id),
    relay_uuid UUID NOT NULL UNIQUE,
    CHECK (entry_node_id <> exit_node_id)
);
CREATE TABLE singbox_policy_nodes (
    group_id BIGINT NOT NULL REFERENCES singbox_policy_groups(id) ON DELETE CASCADE,
    node_id BIGINT NOT NULL REFERENCES nodes(id),
    PRIMARY KEY (group_id, node_id)
);
CREATE TABLE singbox_policy_chains (
    group_id BIGINT NOT NULL REFERENCES singbox_policy_groups(id) ON DELETE CASCADE,
    chain_id BIGINT NOT NULL REFERENCES singbox_chains(id),
    PRIMARY KEY (group_id, chain_id)
);
CREATE TABLE singbox_user_policies (
    user_id BIGINT NOT NULL REFERENCES users(id),
    group_id BIGINT NOT NULL REFERENCES singbox_policy_groups(id),
    PRIMARY KEY (user_id, group_id)
);
CREATE INDEX singbox_user_policies_group_idx ON singbox_user_policies(group_id);

CREATE TABLE singbox_package_groups (
    id BIGSERIAL PRIMARY KEY,
    name TEXT NOT NULL,
    monthly_bytes NUMERIC(20,0) CHECK (monthly_bytes > 0 AND monthly_bytes <= 18446744073709551615),
    reset_day INTEGER NOT NULL CHECK (reset_day BETWEEN 1 AND 31),
    reset_hour INTEGER NOT NULL CHECK (reset_hour BETWEEN 0 AND 23),
    reset_minute INTEGER NOT NULL CHECK (reset_minute BETWEEN 0 AND 59),
    timezone TEXT NOT NULL,
    duration_days INTEGER NOT NULL CHECK (duration_days BETWEEN 1 AND 36500),
    deleted_at BIGINT
);
CREATE TABLE singbox_package_assignments (
    id BIGSERIAL PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES users(id),
    request_id UUID NOT NULL,
    package_group_id BIGINT NOT NULL REFERENCES singbox_package_groups(id),
    package_name TEXT NOT NULL,
    monthly_bytes NUMERIC(20,0) CHECK (monthly_bytes > 0 AND monthly_bytes <= 18446744073709551615),
    reset_day INTEGER NOT NULL CHECK (reset_day BETWEEN 1 AND 31),
    reset_hour INTEGER NOT NULL CHECK (reset_hour BETWEEN 0 AND 23),
    reset_minute INTEGER NOT NULL CHECK (reset_minute BETWEEN 0 AND 59),
    timezone TEXT NOT NULL,
    starts_at BIGINT NOT NULL,
    expires_at BIGINT NOT NULL CHECK (expires_at > starts_at),
    UNIQUE (user_id, request_id),
    UNIQUE (user_id, id)
);
CREATE TABLE singbox_user_packages (
    user_id BIGINT PRIMARY KEY REFERENCES users(id),
    assignment_id BIGINT NOT NULL,
    FOREIGN KEY (user_id, assignment_id) REFERENCES singbox_package_assignments(user_id, id)
);
CREATE INDEX usage_records_user_period_idx ON usage_records(user_id, period_end);

-- Days 29-31 are clamped independently in each local calendar month.
CREATE FUNCTION singbox_cycle_bounds(at_s BIGINT, reset_day INTEGER, reset_hour INTEGER,
    reset_minute INTEGER, zone TEXT)
RETURNS TABLE (cycle_start BIGINT, next_reset BIGINT)
LANGUAGE plpgsql STABLE STRICT AS $$
DECLARE
    month_start TIMESTAMP;
    boundary TIMESTAMP;
    start_instant TIMESTAMPTZ;
    next_instant TIMESTAMPTZ;
BEGIN
    month_start := date_trunc('month', to_timestamp(at_s) AT TIME ZONE zone);
    boundary := month_start + (LEAST(reset_day, EXTRACT(DAY FROM month_start + INTERVAL '1 month - 1 day')::INTEGER) - 1) * INTERVAL '1 day'
        + reset_hour * INTERVAL '1 hour' + reset_minute * INTERVAL '1 minute';
    start_instant := boundary AT TIME ZONE zone;
    IF start_instant > to_timestamp(at_s) THEN
        month_start := month_start - INTERVAL '1 month';
        boundary := month_start + (LEAST(reset_day, EXTRACT(DAY FROM month_start + INTERVAL '1 month - 1 day')::INTEGER) - 1) * INTERVAL '1 day'
            + reset_hour * INTERVAL '1 hour' + reset_minute * INTERVAL '1 minute';
        start_instant := boundary AT TIME ZONE zone;
    END IF;
    month_start := month_start + INTERVAL '1 month';
    boundary := month_start + (LEAST(reset_day, EXTRACT(DAY FROM month_start + INTERVAL '1 month - 1 day')::INTEGER) - 1) * INTERVAL '1 day'
        + reset_hour * INTERVAL '1 hour' + reset_minute * INTERVAL '1 minute';
    next_instant := boundary AT TIME ZONE zone;
    RETURN QUERY SELECT EXTRACT(EPOCH FROM start_instant)::BIGINT,
        EXTRACT(EPOCH FROM next_instant)::BIGINT;
END;
$$;

-- All business timestamps use protocol Unix seconds, unlike dirty_at debounce milliseconds.
-- A batch is charged once to the month containing its exclusive end minus 1 second.
-- Runtime epochs and package assignments never reset the immutable usage ledger.
CREATE FUNCTION singbox_entitlements(at_s BIGINT)
RETURNS TABLE (user_id BIGINT, package_group_id BIGINT, package_name TEXT, monthly_bytes TEXT,
    reset_day INTEGER, reset_hour INTEGER, reset_minute INTEGER, timezone TEXT,
    starts_at BIGINT, expires_at BIGINT, cycle_start BIGINT, next_reset BIGINT,
    used_bytes TEXT, status TEXT, allowed BOOLEAN)
LANGUAGE sql STABLE AS $$
    WITH evaluated AS (
        SELECT u.id, a.package_group_id, a.package_name, a.monthly_bytes,
            a.reset_day, a.reset_hour, a.reset_minute, a.timezone, a.starts_at, a.expires_at,
            b.cycle_start, b.next_reset, COALESCE(t.used, 0) AS used,
            CASE WHEN a.id IS NULL THEN 'unmetered'
                WHEN at_s < a.starts_at THEN 'not_started'
                WHEN at_s >= a.expires_at THEN 'expired'
                WHEN a.monthly_bytes IS NOT NULL AND COALESCE(t.used, 0) >= a.monthly_bytes THEN 'exhausted'
                ELSE 'active' END AS status
        FROM users u
        LEFT JOIN singbox_user_packages p ON p.user_id=u.id
        LEFT JOIN singbox_package_assignments a ON a.id=p.assignment_id
        LEFT JOIN LATERAL singbox_cycle_bounds(at_s, a.reset_day, a.reset_hour, a.reset_minute, a.timezone) b ON TRUE
        LEFT JOIN LATERAL (
            SELECT SUM(r.uplink + r.downlink) AS used FROM usage_records r
            WHERE r.user_id=u.id AND r.period_end > b.cycle_start AND r.period_end <= b.next_reset
        ) t ON TRUE
        WHERE u.deleted_at IS NULL
    )
    SELECT id, package_group_id, package_name, monthly_bytes::TEXT,
        reset_day, reset_hour, reset_minute, timezone, starts_at, expires_at,
        cycle_start, next_reset, used::TEXT, status, status IN ('unmetered', 'active')
    FROM evaluated;
$$;

CREATE VIEW singbox_desired_accesses AS
    SELECT DISTINCT granted.user_id, granted.node_id
    FROM (
        SELECT user_id, node_id FROM accesses WHERE direct_grant
        UNION
        SELECT u.user_id, n.node_id FROM singbox_user_policies u JOIN singbox_policy_nodes n ON n.group_id=u.group_id
        UNION
        SELECT u.user_id, c.entry_node_id FROM singbox_user_policies u
            JOIN singbox_policy_chains p ON p.group_id=u.group_id JOIN singbox_chains c ON c.id=p.chain_id
    ) granted
    JOIN users u ON u.id=granted.user_id AND u.deleted_at IS NULL
    JOIN nodes n ON n.id=granted.node_id AND n.deleted_at IS NULL
    JOIN servers s ON s.id=n.server_id AND s.deleted_at IS NULL
    LEFT JOIN singbox_chains c ON c.entry_node_id=n.id
    WHERE c.id IS NULL OR (n.protocol='vless-reality' AND EXISTS (
        SELECT 1 FROM nodes e JOIN servers es ON es.id=e.server_id
        WHERE e.id=c.exit_node_id AND e.protocol='vless-reality' AND e.deleted_at IS NULL AND es.deleted_at IS NULL
    ));

CREATE FUNCTION singbox_eligible_accesses(at_s BIGINT)
RETURNS TABLE (user_id BIGINT, node_id BIGINT, uuid UUID, stat_name TEXT, credential TEXT)
LANGUAGE sql STABLE AS $$
    SELECT a.user_id, a.node_id, a.uuid, a.stat_name, a.credential FROM accesses a
    JOIN singbox_desired_accesses d USING (user_id, node_id)
    JOIN singbox_entitlements(at_s) e ON e.user_id=a.user_id AND e.allowed;
$$;

CREATE TABLE singbox_entitlement_state (
    user_id BIGINT PRIMARY KEY REFERENCES users(id),
    allowed BOOLEAN NOT NULL
);
CREATE TABLE singbox_chain_state (
    chain_id BIGINT PRIMARY KEY REFERENCES singbox_chains(id) ON DELETE CASCADE,
    available BOOLEAN NOT NULL
);
