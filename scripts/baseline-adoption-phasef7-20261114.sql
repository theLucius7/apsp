-- One-time baseline adoption, phase F step 7: from the baseline of phase F step 2 (checksum
-- 277a1788…, fingerprint 1f00c9b3…) to the one where operations keeps its own state about each
-- cloud account and resource instead of writing it on resources' rows (checksum edd39c0a…,
-- fingerprint ab219b8e…):
--   alicloud_accounts.auto_enabled, limit_gb          -> cloud_account_automation, the
--                                                        account's automatic bandwidth reduction
--   alicloud_resources.auto_enabled, cap_mbps,        -> cloud_resource_automation, the
--     power_policy, power_state, power_checked_at,       resource's automatic bandwidth
--     power_error, next_power_at, manual_hold,           reduction, power automation and CDT
--     threshold_hold, cdt_guard_state,                   guard
--     cdt_guard_error, cdt_guard_next_at
-- Every account and resource, archived ones included, gets its row with the values copied
-- exactly: the stored power policies, power states and CDT guard readings as they are (the
-- guard compares its next reading with the stored high-water mark), the holds and the check
-- times. Each copy is checked row by row before the old columns are dropped, all in this one
-- transaction. The revisions, the refresh schedule and the IPv6 addresses (ipv6_addresses,
-- ipv6_checked_at, ipv6_error describe the instance) stay where they are; the power jobs'
-- account_revision and resource_revision keep referring to them unchanged.
-- The old panel keeps running while this executes and nothing is rolled back afterwards, so
-- alicloud_accounts and alicloud_resources are locked exclusively first: values written before
-- the locks are granted are copied, and none can be written after. Until the new image
-- replaces it, the old panel's statements that read cloud account or resource rows whole or
-- name the moved columns fail and write nothing: its account refresh, power checks, CDT guard,
-- power jobs, bandwidth, address rotation and security group steps stop for those seconds and
-- resume under the new image from the copied state.
-- Work the old panel is carrying out must not be cut in two. A power job or bandwidth
-- operation is 'running' from the transaction that commits its intent until the one that sends
-- the cloud call and records the answer; a security group change while an administrator's
-- request applies its steps. Each next step reads the account and resource rows whole, which
-- fails once the columns are gone, so the work would stay 'running' for good: the new panel's
-- read-back then finds the instance unchanged and keeps the job uncertain, and while it is, every
-- later power job of that resource is refused as busy, the CDT guard's stop included. So the
-- script refuses while any is running; the deploy retries the next minute, once the old panel
-- has finished it. Under the locks the check is final for power jobs and bandwidth operations,
-- whose intent is committed by a transaction that reads the account and resource rows first; a
-- security group change confirmed in the same instant can still slip through (its confirmation
-- does not read them). Address rotations are not checked: every step re-reads the cloud and a
-- step whose result was not stored is simply taken again.
-- Fingerprints are PostgreSQL 16 values with sqlx's _sqlx_migrations present, checked on a
-- fresh initialisation of each baseline.
-- psql -v ON_ERROR_STOP=1 -1 -f scripts/baseline-adoption-phasef7-20261114.sql
SET LOCAL lock_timeout = '5s';
SET LOCAL search_path TO public;
SELECT pg_advisory_xact_lock(73401220);
DO $$
DECLARE actual text;
BEGIN
    IF (SELECT count(*) FROM public._sqlx_migrations) <> 1 OR NOT EXISTS (
        SELECT 1 FROM public._sqlx_migrations WHERE version=1 AND success AND checksum=decode('277a1788abae2b8d4b40b8fa89a5c6339d419129dd4e213c4bc6e4a21eeeca8ee9b5921e65da144eca9738b6f71b79f8','hex')
    ) THEN RAISE EXCEPTION 'expected the phase F step 2 baseline 277a1788; refusing to modify database'; END IF;
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
    IF actual <> '1f00c9b3ee040ba307e844e003f5f15a' OR actual IS DISTINCT FROM (SELECT fingerprint FROM public.apsp_schema_baseline WHERE singleton) THEN
        RAISE EXCEPTION 'existing schema differs from its recorded baseline';
    END IF;
END $$;

-- Creating the foreign keys takes SHARE ROW EXCLUSIVE on alicloud_accounts and
-- alicloud_resources, dropping their columns ACCESS EXCLUSIVE. Both are taken here, once and in
-- this order, so the script never upgrades a lock it holds. Nothing else is changed: the power
-- jobs, bandwidth operations and address rotations stay as they are.
LOCK TABLE public.alicloud_accounts, public.alicloud_resources IN ACCESS EXCLUSIVE MODE;

-- No cloud work the old panel is carrying out may be cut in two (see the header).
DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM public.alicloud_power_jobs WHERE status='running')
        OR EXISTS (SELECT 1 FROM public.alicloud_operations WHERE status='running')
        OR EXISTS (SELECT 1 FROM public.alicloud_security_group_operations WHERE status='running')
    THEN RAISE EXCEPTION 'the old panel is carrying out a power job, bandwidth operation or security group change; retry once it has finished'; END IF;
END $$;

CREATE TABLE public.cloud_account_automation (
    account_id uuid PRIMARY KEY REFERENCES public.alicloud_accounts(id),
    auto_enabled boolean DEFAULT false NOT NULL,
    limit_gb bigint DEFAULT 100 NOT NULL CHECK (((limit_gb >= 1) AND (limit_gb <= 1000000000)))
);

CREATE TABLE public.cloud_resource_automation (
    resource_id uuid PRIMARY KEY REFERENCES public.alicloud_resources(id),
    auto_enabled boolean DEFAULT false NOT NULL,
    cap_mbps bigint DEFAULT 1 NOT NULL CHECK (((cap_mbps >= 1) AND (cap_mbps <= 100))),
    power_policy jsonb DEFAULT '{"enabled": false, "limit_gb": 100, "keepalive": false, "stop_mode": "KeepCharging", "stop_time": "23:00", "start_time": "08:00", "schedule_enabled": false, "threshold_action": "off", "threshold_percent": 95, "utc_offset_minutes": 480}'::jsonb NOT NULL,
    power_state jsonb,
    power_checked_at bigint,
    power_error text,
    next_power_at bigint DEFAULT 0 NOT NULL,
    manual_hold boolean DEFAULT false NOT NULL,
    threshold_hold boolean DEFAULT false NOT NULL,
    cdt_guard_state jsonb,
    cdt_guard_error text,
    cdt_guard_next_at bigint DEFAULT 0 NOT NULL
);

INSERT INTO public.cloud_account_automation(account_id, auto_enabled, limit_gb)
    SELECT id, auto_enabled, limit_gb FROM public.alicloud_accounts;
INSERT INTO public.cloud_resource_automation(resource_id, auto_enabled, cap_mbps, power_policy, power_state, power_checked_at, power_error, next_power_at, manual_hold, threshold_hold, cdt_guard_state, cdt_guard_error, cdt_guard_next_at)
    SELECT id, auto_enabled, cap_mbps, power_policy, power_state, power_checked_at, power_error, next_power_at, manual_hold, threshold_hold, cdt_guard_state, cdt_guard_error, cdt_guard_next_at
    FROM public.alicloud_resources;

DO $$
BEGIN
    IF (SELECT count(*) FROM public.cloud_account_automation) <> (SELECT count(*) FROM public.alicloud_accounts)
        OR EXISTS (
            SELECT 1 FROM public.alicloud_accounts a
            LEFT JOIN public.cloud_account_automation m ON m.account_id=a.id
            WHERE m.account_id IS NULL
                OR m.auto_enabled IS DISTINCT FROM a.auto_enabled
                OR m.limit_gb IS DISTINCT FROM a.limit_gb)
    THEN RAISE EXCEPTION 'copied account automation differs from the account rows'; END IF;
    IF (SELECT count(*) FROM public.cloud_resource_automation) <> (SELECT count(*) FROM public.alicloud_resources)
        OR EXISTS (
            SELECT 1 FROM public.alicloud_resources r
            LEFT JOIN public.cloud_resource_automation m ON m.resource_id=r.id
            WHERE m.resource_id IS NULL
                OR m.auto_enabled IS DISTINCT FROM r.auto_enabled
                OR m.cap_mbps IS DISTINCT FROM r.cap_mbps
                OR md5(m.power_policy::text) IS DISTINCT FROM md5(r.power_policy::text)
                OR md5(m.power_state::text) IS DISTINCT FROM md5(r.power_state::text)
                OR m.power_checked_at IS DISTINCT FROM r.power_checked_at
                OR m.power_error IS DISTINCT FROM r.power_error
                OR m.next_power_at IS DISTINCT FROM r.next_power_at
                OR m.manual_hold IS DISTINCT FROM r.manual_hold
                OR m.threshold_hold IS DISTINCT FROM r.threshold_hold
                OR md5(m.cdt_guard_state::text) IS DISTINCT FROM md5(r.cdt_guard_state::text)
                OR m.cdt_guard_error IS DISTINCT FROM r.cdt_guard_error
                OR m.cdt_guard_next_at IS DISTINCT FROM r.cdt_guard_next_at)
    THEN RAISE EXCEPTION 'copied resource automation differs from the resource rows'; END IF;
END $$;

ALTER TABLE public.alicloud_accounts
    DROP COLUMN auto_enabled,
    DROP COLUMN limit_gb;

ALTER TABLE public.alicloud_resources
    DROP COLUMN auto_enabled,
    DROP COLUMN cap_mbps,
    DROP COLUMN power_policy,
    DROP COLUMN power_state,
    DROP COLUMN power_checked_at,
    DROP COLUMN power_error,
    DROP COLUMN next_power_at,
    DROP COLUMN manual_hold,
    DROP COLUMN threshold_hold,
    DROP COLUMN cdt_guard_state,
    DROP COLUMN cdt_guard_error,
    DROP COLUMN cdt_guard_next_at;

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
    IF actual <> 'ab219b8eacbbc3899de0b9428addfdbc' THEN RAISE EXCEPTION 'result differs from fresh target baseline: %',actual; END IF;
    UPDATE public.apsp_schema_baseline SET fingerprint=actual WHERE singleton;
    UPDATE public._sqlx_migrations SET checksum=decode('edd39c0a103b64646b4602b406b499f83220ffa4dd2a8929ad14bd60345a94a2e9eaec4d7421383885d0898dd645a25c','hex') WHERE version=1;
END $$;
