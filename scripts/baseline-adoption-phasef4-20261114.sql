-- One-time baseline adoption, phase F step 4: from the baseline of phase F step 1 (checksum
-- f9a10013…, fingerprint 5c70bc13…) to the one where observability keeps its own state about
-- each server instead of writing it on the server row (checksum 51ebdbd2…, fingerprint
-- 1ff76742…):
--   servers.telemetry_settings, latest_metrics, metrics_sampled_at -> telemetry_servers
--   servers.probe_revision, probe_fingerprint                      -> probe_assignments
--   servers.quality_refresh_started                                -> server_ip_quality_refreshes
-- Every server, deleted ones included, gets its telemetry and probe assignment row with the
-- values copied exactly (Agents ignore a probe assignment whose revision is lower than one
-- they have seen); a refresh lease row exists only where a lease was recorded. The columns are
-- dropped in the same transaction.
-- The old panel keeps running while this executes and nothing is rolled back afterwards, so
-- the copy and the drop happen in this one transaction under an exclusive lock on servers:
-- values written before the lock is granted are copied, and none can be written to the old
-- columns after. Until the new image replaces it, the old panel's statements that name the
-- dropped columns fail; Agents resend the telemetry batches and probe results it refuses.
-- Fingerprints are PostgreSQL 16 values with sqlx's _sqlx_migrations present, checked on a
-- fresh initialisation of each baseline.
-- psql -v ON_ERROR_STOP=1 -1 -f scripts/baseline-adoption-phasef4-20261114.sql
SET LOCAL lock_timeout = '5s';
SET LOCAL search_path TO public;
SELECT pg_advisory_xact_lock(73401220);
DO $$
DECLARE actual text;
BEGIN
    IF (SELECT count(*) FROM public._sqlx_migrations) <> 1 OR NOT EXISTS (
        SELECT 1 FROM public._sqlx_migrations WHERE version=1 AND success AND checksum=decode('f9a100135e5f9db2fc54a2720f57cf2bd6265ce2d7038796b1c7a1c5785f6f82c559526dcd40b54b0ce77bed21f25bca','hex')
    ) THEN RAISE EXCEPTION 'expected the phase F step 1 baseline f9a10013; refusing to modify database'; END IF;
    SELECT (SELECT md5(string_agg(definition, E'\n' ORDER BY definition COLLATE "C"))
FROM (
    SELECT 'relation:' || c.relname || ':' || c.relkind::text || ':' || a.attname || ':' ||
           row_number() OVER (PARTITION BY a.attrelid ORDER BY a.attnum) || ':' ||
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
    IF actual <> '5c70bc13eb7f87e4e8ffe072cbc2f754' OR actual IS DISTINCT FROM (SELECT fingerprint FROM public.apsp_schema_baseline WHERE singleton) THEN
        RAISE EXCEPTION 'existing schema differs from its recorded baseline';
    END IF;
END $$;

-- Creating the foreign keys takes SHARE ROW EXCLUSIVE on servers and dropping the columns
-- ACCESS EXCLUSIVE; both are taken here, once, so the script never upgrades a lock it holds.
LOCK TABLE public.servers IN ACCESS EXCLUSIVE MODE;

CREATE TABLE public.telemetry_servers (
    server_id bigint PRIMARY KEY REFERENCES public.servers(id) ON DELETE CASCADE,
    settings jsonb DEFAULT '{"persist_interval_secs": 60}'::jsonb NOT NULL,
    latest_metrics jsonb DEFAULT '{}'::jsonb NOT NULL,
    sampled_at bigint DEFAULT 0 NOT NULL
);

CREATE TABLE public.probe_assignments (
    server_id bigint PRIMARY KEY REFERENCES public.servers(id) ON DELETE CASCADE,
    revision bigint DEFAULT 0 NOT NULL CHECK (revision >= 0),
    fingerprint text
);

CREATE TABLE public.server_ip_quality_refreshes (
    server_id bigint PRIMARY KEY REFERENCES public.servers(id) ON DELETE CASCADE,
    started_at bigint NOT NULL
);

INSERT INTO public.telemetry_servers(server_id, settings, latest_metrics, sampled_at)
    SELECT id, telemetry_settings, latest_metrics, metrics_sampled_at FROM public.servers;
INSERT INTO public.probe_assignments(server_id, revision, fingerprint)
    SELECT id, probe_revision, probe_fingerprint FROM public.servers;
INSERT INTO public.server_ip_quality_refreshes(server_id, started_at)
    SELECT id, quality_refresh_started FROM public.servers WHERE quality_refresh_started IS NOT NULL;

DO $$
BEGIN
    IF (SELECT count(*) FROM public.telemetry_servers) <> (SELECT count(*) FROM public.servers)
        OR (SELECT count(*) FROM public.probe_assignments) <> (SELECT count(*) FROM public.servers)
        OR (SELECT count(*) FROM public.server_ip_quality_refreshes) <> (SELECT count(*) FROM public.servers WHERE quality_refresh_started IS NOT NULL)
        OR EXISTS (
            SELECT 1 FROM public.servers s
            LEFT JOIN public.telemetry_servers t ON t.server_id=s.id
            LEFT JOIN public.probe_assignments p ON p.server_id=s.id
            LEFT JOIN public.server_ip_quality_refreshes q ON q.server_id=s.id
            WHERE t.server_id IS NULL OR p.server_id IS NULL
                OR t.settings IS DISTINCT FROM s.telemetry_settings
                OR t.latest_metrics IS DISTINCT FROM s.latest_metrics
                OR t.sampled_at IS DISTINCT FROM s.metrics_sampled_at
                OR p.revision IS DISTINCT FROM s.probe_revision
                OR p.fingerprint IS DISTINCT FROM s.probe_fingerprint
                OR q.started_at IS DISTINCT FROM s.quality_refresh_started)
    THEN RAISE EXCEPTION 'copied values differ from the server rows'; END IF;
END $$;

ALTER TABLE public.servers
    DROP COLUMN latest_metrics,
    DROP COLUMN quality_refresh_started,
    DROP COLUMN metrics_sampled_at,
    DROP COLUMN telemetry_settings,
    DROP COLUMN probe_revision,
    DROP COLUMN probe_fingerprint;

DO $$
DECLARE actual text;
BEGIN
    SELECT (SELECT md5(string_agg(definition, E'\n' ORDER BY definition COLLATE "C"))
FROM (
    SELECT 'relation:' || c.relname || ':' || c.relkind::text || ':' || a.attname || ':' ||
           row_number() OVER (PARTITION BY a.attrelid ORDER BY a.attnum) || ':' ||
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
    IF actual <> '1ff767422097a52e0c7430ad4ce4de7a' THEN RAISE EXCEPTION 'result differs from fresh target baseline: %',actual; END IF;
    UPDATE public.apsp_schema_baseline SET fingerprint=actual WHERE singleton;
    UPDATE public._sqlx_migrations SET checksum=decode('51ebdbd2bb12a56bed2cef4ee29824deaa1cc024106129dae2d48b61265fae9d96afd57f8dc31ea20449e778495bfa2b','hex') WHERE version=1;
END $$;
