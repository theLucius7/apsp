-- Members served through the compatibility bridge are metered by the legacy
-- nodes, but their traffic on natively managed nodes counts too. Both meters see
-- different nodes, except taken-over nodes whose raw records are also queued
-- for legacy delivery. Exclude every queued record from the native addition,
-- even before delivery: the bridge alone applies its multiplier and cycle split.
CREATE INDEX singbox_nexus_usage_user_period ON singbox_nexus_usage_deliveries(user_id,period_end);
CREATE OR REPLACE FUNCTION singbox_entitlements(at_s BIGINT)
RETURNS TABLE (user_id BIGINT, package_group_id BIGINT, package_name TEXT, monthly_bytes TEXT,
    reset_day INTEGER, reset_hour INTEGER, reset_minute INTEGER, timezone TEXT,
    starts_at BIGINT, expires_at BIGINT, cycle_start BIGINT, next_reset BIGINT,
    used_bytes TEXT, status TEXT, allowed BOOLEAN)
LANGUAGE sql STABLE AS $$
    WITH evaluated AS (
        SELECT u.id,a.package_group_id,a.package_name,a.monthly_bytes,a.reset_day,a.reset_hour,a.reset_minute,a.timezone,a.starts_at,a.expires_at,
            b.cycle_start,b.next_reset,GREATEST(COALESCE(t.used,0)-COALESCE(c.credit,0),0) AS used,
            CASE WHEN m.state IS NOT NULL AND (m.state<>'active' OR (m.sponsor_required AND m.first_purchase_at IS NULL)) THEN CASE WHEN m.state='active' THEN 'pending' ELSE m.state END
                WHEN a.id IS NULL AND bridge.enabled THEN 'no_package' WHEN a.id IS NULL THEN 'unmetered' WHEN at_s<a.starts_at THEN 'not_started' WHEN at_s>=a.expires_at THEN 'expired'
                WHEN a.monthly_bytes IS NOT NULL AND GREATEST(COALESCE(t.used,0)-COALESCE(c.credit,0),0)>=a.monthly_bytes THEN 'exhausted' ELSE 'active' END AS status
        FROM users u LEFT JOIN singbox_members m ON m.user_id=u.id LEFT JOIN singbox_user_packages p ON p.user_id=u.id
        LEFT JOIN singbox_package_assignments a ON a.id=p.assignment_id
        LEFT JOIN singbox_nexus_identities identity ON identity.user_id=u.id
        LEFT JOIN singbox_nexus_bridge bridge ON bridge.import_id=identity.import_id AND bridge.enabled
        LEFT JOIN singbox_nexus_metering meter ON meter.user_id=u.id AND bridge.singleton
        LEFT JOIN LATERAL singbox_cycle_bounds(at_s,a.reset_day,a.reset_hour,a.reset_minute,a.timezone) native ON TRUE
        LEFT JOIN LATERAL (SELECT CASE WHEN meter.user_id IS NOT NULL THEN COALESCE(meter.last_reset_at,0) ELSE native.cycle_start END AS cycle_start,CASE WHEN meter.user_id IS NOT NULL THEN COALESCE(meter.next_reset_at,253402300799) ELSE native.next_reset END AS next_reset) b ON TRUE
        LEFT JOIN LATERAL (SELECT CASE WHEN meter.user_id IS NOT NULL THEN meter.upload+meter.download+GREATEST(COALESCE(singbox_usage_window(u.id,b.cycle_start,b.next_reset),0)-COALESCE((SELECT SUM(d.uplink+d.downlink) FROM singbox_nexus_usage_deliveries d WHERE d.user_id=u.id AND d.period_end>b.cycle_start AND d.period_end<=b.next_reset),0),0) ELSE singbox_usage_window(u.id,b.cycle_start,b.next_reset) END AS used) t ON TRUE
        LEFT JOIN LATERAL (SELECT SUM(q.bytes) AS credit FROM singbox_quota_credits q WHERE q.user_id=u.id AND q.cycle_start=b.cycle_start AND q.next_reset=b.next_reset) c ON TRUE
        WHERE u.deleted_at IS NULL
    )
    SELECT id,package_group_id,package_name,monthly_bytes::TEXT,reset_day,reset_hour,reset_minute,timezone,starts_at,NULLIF(expires_at,253402300799),cycle_start,next_reset,used::TEXT,status,status IN('unmetered','active') FROM evaluated;
$$;
