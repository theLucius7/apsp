-- Derived per-UTC-day totals of the immutable proxy usage ledger (ADR 0083).
-- usage_records stays the source of truth; this table only speeds up reads.
CREATE TABLE singbox_usage_daily (
    user_id BIGINT NOT NULL REFERENCES users(id),
    day BIGINT NOT NULL CHECK (day % 86400 = 0),
    node_id BIGINT NOT NULL REFERENCES nodes(id),
    uplink NUMERIC NOT NULL CHECK (uplink >= 0),
    downlink NUMERIC NOT NULL CHECK (downlink >= 0),
    records BIGINT NOT NULL CHECK (records > 0),
    last_period_end BIGINT NOT NULL,
    PRIMARY KEY (user_id, day, node_id)
);
CREATE INDEX singbox_usage_daily_day_idx ON singbox_usage_daily(day);
CREATE INDEX singbox_usage_daily_node_idx ON singbox_usage_daily(node_id, day);

-- Statement-level so a multi-row insert is aggregated once; rows skipped by
-- ON CONFLICT DO NOTHING are absent from the transition table.
CREATE FUNCTION singbox_usage_daily_apply() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    INSERT INTO singbox_usage_daily(user_id, day, node_id, uplink, downlink, records, last_period_end)
    SELECT user_id, period_end / 86400 * 86400, node_id, SUM(uplink), SUM(downlink), COUNT(*), MAX(period_end)
    FROM inserted
    GROUP BY user_id, period_end / 86400 * 86400, node_id
    ON CONFLICT (user_id, day, node_id) DO UPDATE SET
        uplink = singbox_usage_daily.uplink + EXCLUDED.uplink,
        downlink = singbox_usage_daily.downlink + EXCLUDED.downlink,
        records = singbox_usage_daily.records + EXCLUDED.records,
        last_period_end = GREATEST(singbox_usage_daily.last_period_end, EXCLUDED.last_period_end);
    RETURN NULL;
END;
$$;

CREATE TRIGGER singbox_usage_daily_insert AFTER INSERT ON usage_records
    REFERENCING NEW TABLE AS inserted
    FOR EACH STATEMENT EXECUTE FUNCTION singbox_usage_daily_apply();

-- Recomputes every day from the ledger; used for the initial backfill and
-- for an operator-confirmed repair. Blocks concurrent ledger inserts meanwhile.
CREATE FUNCTION singbox_usage_daily_rebuild() RETURNS BIGINT LANGUAGE plpgsql AS $$
DECLARE
    rebuilt BIGINT;
BEGIN
    LOCK TABLE usage_records IN SHARE ROW EXCLUSIVE MODE;
    DELETE FROM singbox_usage_daily;
    INSERT INTO singbox_usage_daily(user_id, day, node_id, uplink, downlink, records, last_period_end)
    SELECT user_id, period_end / 86400 * 86400, node_id, SUM(uplink), SUM(downlink), COUNT(*), MAX(period_end)
    FROM usage_records
    GROUP BY user_id, period_end / 86400 * 86400, node_id;
    GET DIAGNOSTICS rebuilt = ROW_COUNT;
    RETURN rebuilt;
END;
$$;

SELECT singbox_usage_daily_rebuild();

-- Exact SUM(uplink + downlink) of a user's records with after < period_end <= until.
-- Whole UTC days come from the rollup; at most one partial day at each end is
-- read from the ledger through usage_records_user_period_idx.
CREATE FUNCTION singbox_usage_window(p_user BIGINT, p_after BIGINT, p_until BIGINT)
RETURNS NUMERIC LANGUAGE sql STABLE STRICT AS $$
    WITH bounds AS (
        SELECT (ceil((p_after + 1)::NUMERIC / 86400) * 86400)::BIGINT AS lo,
            (floor((p_until + 1)::NUMERIC / 86400) * 86400)::BIGINT AS hi
    )
    SELECT CASE
        WHEN p_until <= p_after THEN 0::NUMERIC
        -- Day buckets truncate toward zero, so negative instants use the ledger only.
        WHEN p_after < 0 THEN COALESCE((SELECT SUM(r.uplink + r.downlink) FROM usage_records r
            WHERE r.user_id = p_user AND r.period_end > p_after AND r.period_end <= p_until), 0)
        ELSE COALESCE((SELECT SUM(d.uplink + d.downlink) FROM singbox_usage_daily d
                WHERE d.user_id = p_user AND d.day >= b.lo AND d.day < b.hi), 0)
            + COALESCE((SELECT SUM(r.uplink + r.downlink) FROM usage_records r
                WHERE r.user_id = p_user AND r.period_end > p_after
                    AND r.period_end < LEAST(b.lo, p_until + 1)), 0)
            + COALESCE((SELECT SUM(r.uplink + r.downlink) FROM usage_records r
                WHERE r.user_id = p_user AND r.period_end >= GREATEST(b.lo, b.hi)
                    AND r.period_end <= p_until), 0)
        END
    FROM bounds b
$$;

-- Preserve 0052's explicit quota credits; only ledger reads use the rollup.
CREATE OR REPLACE FUNCTION singbox_entitlements(at_s BIGINT)
RETURNS TABLE (user_id BIGINT, package_group_id BIGINT, package_name TEXT, monthly_bytes TEXT,
    reset_day INTEGER, reset_hour INTEGER, reset_minute INTEGER, timezone TEXT,
    starts_at BIGINT, expires_at BIGINT, cycle_start BIGINT, next_reset BIGINT,
    used_bytes TEXT, status TEXT, allowed BOOLEAN)
LANGUAGE sql STABLE AS $$
    WITH evaluated AS (
        SELECT u.id, a.package_group_id, a.package_name, a.monthly_bytes,
            a.reset_day, a.reset_hour, a.reset_minute, a.timezone, a.starts_at, a.expires_at,
            b.cycle_start, b.next_reset,
            GREATEST(COALESCE(t.used, 0) - COALESCE(c.credit, 0), 0) AS used,
            CASE WHEN a.id IS NULL THEN 'unmetered'
                WHEN at_s < a.starts_at THEN 'not_started'
                WHEN at_s >= a.expires_at THEN 'expired'
                WHEN a.monthly_bytes IS NOT NULL
                    AND GREATEST(COALESCE(t.used, 0) - COALESCE(c.credit, 0), 0) >= a.monthly_bytes THEN 'exhausted'
                ELSE 'active' END AS status
        FROM users u
        LEFT JOIN singbox_user_packages p ON p.user_id=u.id
        LEFT JOIN singbox_package_assignments a ON a.id=p.assignment_id
        LEFT JOIN LATERAL singbox_cycle_bounds(at_s, a.reset_day, a.reset_hour, a.reset_minute, a.timezone) b ON TRUE
        LEFT JOIN LATERAL (
            SELECT singbox_usage_window(u.id, b.cycle_start, b.next_reset) AS used
        ) t ON TRUE
        LEFT JOIN LATERAL (
            SELECT SUM(q.bytes) AS credit FROM singbox_quota_credits q
            WHERE q.user_id=u.id AND q.cycle_start=b.cycle_start AND q.next_reset=b.next_reset
        ) c ON TRUE
        WHERE u.deleted_at IS NULL
    )
    SELECT id, package_group_id, package_name, monthly_bytes::TEXT,
        reset_day, reset_hour, reset_minute, timezone, starts_at, expires_at,
        cycle_start, next_reset, used::TEXT, status, status IN ('unmetered', 'active')
    FROM evaluated;
$$;
