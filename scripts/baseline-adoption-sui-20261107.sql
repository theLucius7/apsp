-- One-time baseline adoption for the S-UI capability alignment.
-- Validated on PostgreSQL 18 against the previous source baseline only.
-- Back up and verify the actual production schema first, stop the panel, then
-- apply this transaction before deploying the matching new panel binary.
-- psql -v ON_ERROR_STOP=1 -1 -f scripts/baseline-adoption-sui-20261107.sql
SET LOCAL lock_timeout = '10s';
SET LOCAL search_path TO public;
SELECT pg_advisory_xact_lock(73401220);
DO $$
DECLARE actual text;
BEGIN
    IF (SELECT count(*) FROM public._sqlx_migrations) <> 1 OR NOT EXISTS (
        SELECT 1 FROM public._sqlx_migrations WHERE version=1 AND success AND checksum=decode('5e0ed43b555091e715ba2641438748aec06b18fb7c8682542756649cd500e14ebfe2914950907677ee573c509292ad77','hex')
    ) THEN RAISE EXCEPTION 'expected the baseline at 287de2b2; refusing to modify database'; END IF;
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
    IF actual <> '48047f10471ff2f949b3c5959fb85e22' OR actual IS DISTINCT FROM (SELECT fingerprint FROM public.apsp_schema_baseline WHERE singleton) THEN
        RAISE EXCEPTION 'existing schema differs from its recorded baseline';
    END IF;
END $$;

CREATE TABLE public.singbox_user_controls (
    user_id BIGINT PRIMARY KEY REFERENCES public.users(id),
    paused BOOLEAN NOT NULL DEFAULT FALSE,
    revision BIGINT NOT NULL DEFAULT 1 CHECK(revision>0),
    updated_at BIGINT NOT NULL
);

CREATE TABLE public.singbox_runtime_controls (server_id BIGINT PRIMARY KEY REFERENCES public.servers(id), control_secret TEXT NOT NULL);
INSERT INTO public.singbox_runtime_controls(server_id,control_secret) SELECT id,gen_random_uuid()::text || gen_random_uuid()::text FROM public.servers;

CREATE TABLE public.singbox_server_profiles (
    server_id BIGINT PRIMARY KEY REFERENCES public.servers(id),
    definition JSONB NOT NULL,
    revision BIGINT NOT NULL CHECK (revision > 0),
    updated_at BIGINT NOT NULL
);

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
        LEFT JOIN LATERAL (SELECT CASE WHEN meter.user_id IS NOT NULL THEN meter.upload+meter.download+GREATEST(COALESCE(singbox_usage_window(u.id,b.cycle_start,b.next_reset),0)-COALESCE((SELECT SUM(d.uplink+d.downlink) FROM singbox_nexus_usage_deliveries d WHERE d.user_id=u.id AND d.period_end>b.cycle_start AND d.period_end<=b.next_reset),0),0) ELSE singbox_usage_window(u.id,b.cycle_start,b.next_reset) END AS used) t ON TRUE
        LEFT JOIN LATERAL (SELECT SUM(q.bytes) AS credit FROM singbox_quota_credits q WHERE q.user_id=u.id AND q.cycle_start=b.cycle_start AND q.next_reset=b.next_reset) c ON TRUE
        WHERE u.deleted_at IS NULL
    )
    SELECT id,package_group_id,package_name,monthly_bytes::TEXT,reset_day,reset_hour,reset_minute,timezone,starts_at,NULLIF(expires_at,253402300799),cycle_start,next_reset,used::TEXT,status,status IN('unmetered','active') FROM evaluated;
$$;
ALTER TABLE public.nodes DROP CONSTRAINT nodes_protocol_check;
ALTER TABLE public.nodes ADD CONSTRAINT nodes_protocol_check CHECK ((protocol = ANY (ARRAY['vless-reality'::text, 'vless-tunnel'::text, 'hysteria2'::text, 'shadowsocks2022'::text, 'tuic'::text, 'anytls'::text, 'naive'::text, 'snell-v6'::text, 'vless-tls'::text, 'trojan'::text, 'vmess'::text, 'http'::text, 'mixed'::text, 'socks'::text, 'shadowsocks'::text, 'shadowtls'::text])));
ALTER TABLE public.singbox_client_templates DROP CONSTRAINT singbox_client_templates_client_version_check;
UPDATE public.singbox_client_templates SET client_version='1.14.4';
ALTER TABLE public.singbox_client_templates ADD CONSTRAINT singbox_client_templates_client_version_check CHECK ((client_version = '1.14.4'::text));
ALTER TABLE public.singbox_chain_runtime_requirements DROP CONSTRAINT singbox_chain_runtime_requirements_runtime_version_check;
ALTER TABLE public.singbox_chain_runtime_requirements ADD CONSTRAINT singbox_chain_runtime_requirements_runtime_version_check CHECK ((runtime_version = ANY (ARRAY['1.14.2'::text, '1.14.2-apsp.1'::text, '1.14.0-apsp.1'::text, '1.14.4-apsp.1'::text, '1.14.4-apsp.2'::text])));
ALTER TABLE public.singbox_runtime_manifest_facts DROP CONSTRAINT singbox_runtime_manifest_facts_runtime_version_check;
ALTER TABLE public.singbox_runtime_manifest_facts ADD CONSTRAINT singbox_runtime_manifest_facts_runtime_version_check CHECK ((runtime_version = ANY (ARRAY['1.14.2'::text, '1.14.2-apsp.1'::text, '1.14.0-apsp.1'::text, '1.14.4-apsp.1'::text, '1.14.4-apsp.2'::text])));
ALTER TABLE public.singbox_runtime_rollouts DROP CONSTRAINT singbox_runtime_rollouts_runtime_version_check;
ALTER TABLE public.singbox_runtime_rollouts ADD CONSTRAINT singbox_runtime_rollouts_runtime_version_check CHECK ((runtime_version = ANY (ARRAY['1.14.2'::text, '1.14.2-apsp.1'::text, '1.14.0-apsp.1'::text, '1.14.4-apsp.1'::text, '1.14.4-apsp.2'::text])));

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
    IF actual <> '719b65a0dc7c564a3873d4322319625a' THEN RAISE EXCEPTION 'result differs from fresh target baseline: %',actual; END IF;
    UPDATE public.apsp_schema_baseline SET fingerprint=actual WHERE singleton;
    UPDATE public._sqlx_migrations SET checksum=decode('405612a777a144b2f00f90bd2446dca71e3b754c5b1e5625249469ec274fe1af75b2171a15b720882beca2630b69f1a7','hex') WHERE version=1;
END $$;
