-- One-time baseline adoption: usage delivered to the compatibility bridge gets a daily rollup,
-- like the native ledger's singbox_usage_daily, so evaluating every user's entitlement no longer
-- sums each metered user's whole cycle of delivery rows (about 208,000 rows, most of an
-- evaluation's time, on every publication cycle). singbox_cycle_bounds also states that it
-- returns one row. From the baseline of 2026-11-10 (checksum a1c4b16e…, fingerprint bd3c95e0…)
-- to the new one (43294f43…, 489f47a3…).
-- Fingerprints are PostgreSQL 16 values with sqlx's _sqlx_migrations present, checked on a fresh
-- initialisation of each baseline. Deliveries are only ever inserted (their byte counts never
-- change and rows are never deleted), so the rollup is filled once from the existing rows while
-- inserts wait, and the trigger keeps it from then on.
-- psql -v ON_ERROR_STOP=1 -1 -f scripts/baseline-adoption-delivery-daily-20261113.sql
SET LOCAL lock_timeout = '10s';
SET LOCAL search_path TO public;
SELECT pg_advisory_xact_lock(73401220);
DO $$
DECLARE actual text;
BEGIN
    IF (SELECT count(*) FROM public._sqlx_migrations) <> 1 OR NOT EXISTS (
        SELECT 1 FROM public._sqlx_migrations WHERE version=1 AND success AND checksum=decode('a1c4b16eed22316dc8bbe6a80f3ab651267305bb3efce631ca621e4978be764d65f172727ae032abf3eb27b3c90fa416','hex')
    ) THEN RAISE EXCEPTION 'expected the baseline a1c4b16e; refusing to modify database'; END IF;
    SELECT (SELECT md5(string_agg(definition, E'\n' ORDER BY definition COLLATE "C"))
FROM (
    SELECT 'relation:' || c.relname || ':' || c.relkind::text || ':' || a.attname || ':' || a.attnum || ':' ||
           format_type(a.atttypid,a.atttypmod) || ':' || a.attnotnull || ':' ||
           COALESCE(pg_get_expr(d.adbin,d.adrelid),'') AS definition
    FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace
    JOIN pg_attribute a ON a.attrelid=c.oid AND a.attnum>0 AND NOT a.attisdropped
    LEFT JOIN pg_attrdef d ON d.adrelid=c.oid AND d.adnum=a.attnum
    WHERE n.nspname='public' AND c.relname <> '_sqlx_migrations'
    UNION ALL
    SELECT 'constraint:' || c.relname || ':' || k.conname || ':' || pg_get_constraintdef(k.oid)
    FROM pg_constraint k JOIN pg_class c ON c.oid=k.conrelid JOIN pg_namespace n ON n.oid=c.relnamespace
    WHERE n.nspname='public' AND c.relname <> '_sqlx_migrations'
    UNION ALL
    SELECT 'index:' || indexname || ':' || indexdef FROM pg_indexes
    WHERE schemaname='public' AND tablename <> '_sqlx_migrations'
    UNION ALL
    SELECT 'view:' || viewname || ':' || definition FROM pg_views WHERE schemaname='public'
    UNION ALL
    SELECT 'function:' || p.proname || ':' || pg_get_functiondef(p.oid)
    FROM pg_proc p JOIN pg_namespace n ON n.oid=p.pronamespace WHERE n.nspname='public' AND p.prokind='f'
    UNION ALL
    SELECT 'trigger:' || c.relname || ':' || pg_get_triggerdef(t.oid)
    FROM pg_trigger t JOIN pg_class c ON c.oid=t.tgrelid JOIN pg_namespace n ON n.oid=c.relnamespace
    WHERE n.nspname='public' AND NOT t.tgisinternal
) AS definitions) INTO actual;
    IF actual <> 'bd3c95e07e55817147441f65c6ba7f47' OR actual IS DISTINCT FROM (SELECT fingerprint FROM public.apsp_schema_baseline WHERE singleton) THEN
        RAISE EXCEPTION 'existing schema differs from its recorded baseline';
    END IF;
END $$;

-- New deliveries wait until the rollup and its trigger exist.
LOCK TABLE public.singbox_nexus_usage_deliveries IN SHARE ROW EXCLUSIVE MODE;

CREATE TABLE public.singbox_nexus_usage_delivery_daily (
    user_id bigint NOT NULL,
    day bigint NOT NULL,
    uplink numeric NOT NULL,
    downlink numeric NOT NULL,
    records bigint NOT NULL,
    CONSTRAINT singbox_nexus_usage_delivery_daily_day_check CHECK (((day % (86400)::bigint) = 0)),
    CONSTRAINT singbox_nexus_usage_delivery_daily_records_check CHECK ((records > 0))
);

ALTER TABLE ONLY public.singbox_nexus_usage_delivery_daily
    ADD CONSTRAINT singbox_nexus_usage_delivery_daily_pkey PRIMARY KEY (user_id, day);

CREATE FUNCTION public.singbox_nexus_delivery_window(p_user bigint, p_after bigint, p_until bigint) RETURNS numeric
    LANGUAGE sql STABLE STRICT
    AS $$
    WITH bounds AS (
        SELECT (ceil((p_after + 1)::NUMERIC / 86400) * 86400)::BIGINT AS lo,
            (floor((p_until + 1)::NUMERIC / 86400) * 86400)::BIGINT AS hi
    )
    SELECT CASE
        WHEN p_until <= p_after THEN 0::NUMERIC
        -- Day buckets truncate toward zero, so negative instants use the deliveries only.
        WHEN p_after < 0 THEN COALESCE((SELECT SUM(d.uplink + d.downlink) FROM singbox_nexus_usage_deliveries d
            WHERE d.user_id = p_user AND d.period_end > p_after AND d.period_end <= p_until), 0)
        ELSE COALESCE((SELECT SUM(r.uplink + r.downlink) FROM singbox_nexus_usage_delivery_daily r
                WHERE r.user_id = p_user AND r.day >= b.lo AND r.day < b.hi), 0)
            + COALESCE((SELECT SUM(d.uplink + d.downlink) FROM singbox_nexus_usage_deliveries d
                WHERE d.user_id = p_user AND d.period_end > p_after
                    AND d.period_end < LEAST(b.lo, p_until + 1)), 0)
            + COALESCE((SELECT SUM(d.uplink + d.downlink) FROM singbox_nexus_usage_deliveries d
                WHERE d.user_id = p_user AND d.period_end >= GREATEST(b.lo, b.hi)
                    AND d.period_end <= p_until), 0)
        END
    FROM bounds b
$$;

CREATE FUNCTION public.singbox_nexus_usage_delivery_daily_apply() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    INSERT INTO singbox_nexus_usage_delivery_daily(user_id, day, uplink, downlink, records)
    SELECT user_id, period_end / 86400 * 86400, SUM(uplink), SUM(downlink), COUNT(*)
    FROM inserted
    GROUP BY user_id, period_end / 86400 * 86400
    ON CONFLICT (user_id, day) DO UPDATE SET
        uplink = singbox_nexus_usage_delivery_daily.uplink + EXCLUDED.uplink,
        downlink = singbox_nexus_usage_delivery_daily.downlink + EXCLUDED.downlink,
        records = singbox_nexus_usage_delivery_daily.records + EXCLUDED.records;
    RETURN NULL;
END;
$$;

INSERT INTO public.singbox_nexus_usage_delivery_daily(user_id, day, uplink, downlink, records)
SELECT user_id, period_end / 86400 * 86400, SUM(uplink), SUM(downlink), COUNT(*)
FROM public.singbox_nexus_usage_deliveries
GROUP BY user_id, period_end / 86400 * 86400;

CREATE TRIGGER singbox_nexus_usage_delivery_daily_insert AFTER INSERT ON public.singbox_nexus_usage_deliveries REFERENCING NEW TABLE AS inserted FOR EACH STATEMENT EXECUTE FUNCTION public.singbox_nexus_usage_delivery_daily_apply();

CREATE OR REPLACE FUNCTION public.singbox_entitlements(at_s bigint) RETURNS TABLE(user_id bigint, package_group_id bigint, package_name text, monthly_bytes text, reset_day integer, reset_hour integer, reset_minute integer, timezone text, starts_at bigint, expires_at bigint, cycle_start bigint, next_reset bigint, used_bytes text, status text, allowed boolean)
    LANGUAGE sql STABLE
    AS $$
    WITH evaluated AS (
        SELECT u.id,a.package_group_id,a.package_name,a.monthly_bytes,a.reset_day,a.reset_hour,a.reset_minute,a.timezone,a.starts_at,a.expires_at,
            b.cycle_start,b.next_reset,GREATEST(COALESCE(t.used,0)-COALESCE(c.credit,0),0) AS used,
            CASE WHEN EXISTS(SELECT 1 FROM singbox_user_controls control WHERE control.user_id=u.id AND control.paused) THEN 'paused' WHEN m.state IS NOT NULL AND (m.state<>'active' OR (m.sponsor_required AND m.first_purchase_at IS NULL)) THEN CASE WHEN m.state='active' THEN 'pending' ELSE m.state END
                WHEN a.id IS NULL AND bridge.enabled THEN 'no_package' WHEN a.id IS NULL THEN 'unmetered' WHEN at_s<a.starts_at THEN 'not_started' WHEN at_s>=a.expires_at THEN 'expired'
                WHEN a.monthly_bytes IS NOT NULL AND GREATEST(COALESCE(t.used,0)-COALESCE(c.credit,0),0)>=a.monthly_bytes THEN 'exhausted' ELSE 'active' END AS status
        FROM users u LEFT JOIN singbox_members m ON m.user_id=u.id LEFT JOIN singbox_user_packages p ON p.user_id=u.id
        LEFT JOIN singbox_package_assignments a ON a.id=p.assignment_id
        LEFT JOIN singbox_nexus_identities identity ON identity.user_id=u.id
        LEFT JOIN singbox_nexus_bridge bridge ON bridge.import_id=identity.import_id AND bridge.enabled
        LEFT JOIN singbox_nexus_metering meter ON meter.user_id=u.id AND bridge.singleton
        LEFT JOIN LATERAL singbox_cycle_bounds(at_s,a.reset_day,a.reset_hour,a.reset_minute,a.timezone) native ON TRUE
        LEFT JOIN LATERAL (SELECT CASE WHEN meter.user_id IS NOT NULL THEN COALESCE(meter.last_reset_at,0) ELSE native.cycle_start END AS cycle_start,CASE WHEN meter.user_id IS NOT NULL THEN COALESCE(meter.next_reset_at,253402300799) ELSE native.next_reset END AS next_reset) b ON TRUE
        LEFT JOIN LATERAL (SELECT CASE WHEN meter.user_id IS NOT NULL THEN meter.upload+meter.download+GREATEST(COALESCE(singbox_usage_window(u.id,b.cycle_start,b.next_reset),0)-COALESCE(singbox_nexus_delivery_window(u.id,b.cycle_start,b.next_reset),0),0) ELSE singbox_usage_window(u.id,b.cycle_start,b.next_reset) END AS used) t ON TRUE
        LEFT JOIN LATERAL (SELECT SUM(q.bytes) AS credit FROM singbox_quota_credits q WHERE q.user_id=u.id AND q.cycle_start=b.cycle_start AND q.next_reset=b.next_reset) c ON TRUE
        WHERE u.deleted_at IS NULL
    )
    SELECT id,package_group_id,package_name,monthly_bytes::TEXT,reset_day,reset_hour,reset_minute,timezone,starts_at,NULLIF(expires_at,253402300799),cycle_start,next_reset,used::TEXT,status,status IN('unmetered','active') FROM evaluated;
$$;

CREATE OR REPLACE FUNCTION public.singbox_cycle_bounds(at_s bigint, reset_day integer, reset_hour integer, reset_minute integer, zone text) RETURNS TABLE(cycle_start bigint, next_reset bigint)
    LANGUAGE plpgsql STABLE STRICT ROWS 1
    AS $$
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

DO $$
DECLARE actual text;
BEGIN
    SELECT (SELECT md5(string_agg(definition, E'\n' ORDER BY definition COLLATE "C"))
FROM (
    SELECT 'relation:' || c.relname || ':' || c.relkind::text || ':' || a.attname || ':' || a.attnum || ':' ||
           format_type(a.atttypid,a.atttypmod) || ':' || a.attnotnull || ':' ||
           COALESCE(pg_get_expr(d.adbin,d.adrelid),'') AS definition
    FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace
    JOIN pg_attribute a ON a.attrelid=c.oid AND a.attnum>0 AND NOT a.attisdropped
    LEFT JOIN pg_attrdef d ON d.adrelid=c.oid AND d.adnum=a.attnum
    WHERE n.nspname='public' AND c.relname <> '_sqlx_migrations'
    UNION ALL
    SELECT 'constraint:' || c.relname || ':' || k.conname || ':' || pg_get_constraintdef(k.oid)
    FROM pg_constraint k JOIN pg_class c ON c.oid=k.conrelid JOIN pg_namespace n ON n.oid=c.relnamespace
    WHERE n.nspname='public' AND c.relname <> '_sqlx_migrations'
    UNION ALL
    SELECT 'index:' || indexname || ':' || indexdef FROM pg_indexes
    WHERE schemaname='public' AND tablename <> '_sqlx_migrations'
    UNION ALL
    SELECT 'view:' || viewname || ':' || definition FROM pg_views WHERE schemaname='public'
    UNION ALL
    SELECT 'function:' || p.proname || ':' || pg_get_functiondef(p.oid)
    FROM pg_proc p JOIN pg_namespace n ON n.oid=p.pronamespace WHERE n.nspname='public' AND p.prokind='f'
    UNION ALL
    SELECT 'trigger:' || c.relname || ':' || pg_get_triggerdef(t.oid)
    FROM pg_trigger t JOIN pg_class c ON c.oid=t.tgrelid JOIN pg_namespace n ON n.oid=c.relnamespace
    WHERE n.nspname='public' AND NOT t.tgisinternal
) AS definitions) INTO actual;
    IF actual <> '489f47a35ac796d10829dbae3745ee86' THEN RAISE EXCEPTION 'result differs from fresh target baseline: %',actual; END IF;
    UPDATE public.apsp_schema_baseline SET fingerprint=actual WHERE singleton;
    UPDATE public._sqlx_migrations SET checksum=decode('43294f430f1a92082af261abd54f0db10986a5d60a9bd3a67055975e1f8cbe8b206f06500cb04993dafcf8bd679df0ee','hex') WHERE version=1;
END $$;
