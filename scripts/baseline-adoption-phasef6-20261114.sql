-- One-time baseline adoption, phase F step 6: from the baseline of phase F step 5 (checksum
-- 345adb3b…, fingerprint 09938605…) to the one where costs keeps its own state about each
-- cloud account and resource instead of writing it on resources' rows, and cloud accounts no
-- longer hold inline access keys (checksum 4e5b8783…, fingerprint 6a79e639…):
--   alicloud_accounts.billing_enabled, balance_enabled,  -> cloud_account_billing; error_code,
--     bill, error_code, traffic, traffic_error,             which only ever held the CDT bill's
--     traffic_history_identity, balance,                   error, is its bill_error
--     balance_error, balance_next_at
--   alicloud_resources.instance_bill, bill_error,        -> cloud_resource_billing
--     bill_next_at
--   alicloud_accounts.access_key_id, access_key_secret     dropped: accounts reference the
--                                                           credential store (credential_id);
--                                                           the script refuses to run while any
--                                                           row still holds a key
-- Every account and resource, archived ones included, gets its row with the values copied
-- exactly (the CDT history identity too, or the next reading would clear the history). Each
-- copy is checked row by row before the old columns are dropped, all in this one transaction.
-- The account's revision, refresh schedule (next_run_at, last_attempt_at), limit_gb and the
-- power columns stay where they are.
-- The old panel keeps running while this executes and nothing is rolled back afterwards, so
-- alicloud_accounts and alicloud_resources are locked exclusively first: values written before
-- the locks are granted are copied, and none can be written after. Until the new image
-- replaces it, the old panel's statements that read cloud account or resource rows whole fail
-- and write nothing: its account refresh, bills, balance, power, bandwidth, address rotation and
-- security group steps stop for those seconds and resume under the new image.
-- Fingerprints are PostgreSQL 16 values with sqlx's _sqlx_migrations present, checked on a
-- fresh initialisation of each baseline.
-- psql -v ON_ERROR_STOP=1 -1 -f scripts/baseline-adoption-phasef6-20261114.sql
SET LOCAL lock_timeout = '5s';
SET LOCAL search_path TO public;
SELECT pg_advisory_xact_lock(73401220);
DO $$
DECLARE actual text;
BEGIN
    IF (SELECT count(*) FROM public._sqlx_migrations) <> 1 OR NOT EXISTS (
        SELECT 1 FROM public._sqlx_migrations WHERE version=1 AND success AND checksum=decode('345adb3b4c0fd71c74fbc90f59b675ea10847936c20f764e7f471af9dff8787964a4d882e4eb83b44cb638d313653aed','hex')
    ) THEN RAISE EXCEPTION 'expected the phase F step 5 baseline 345adb3b; refusing to modify database'; END IF;
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
    IF actual <> '099386056a68204d754deec1fa50340e' OR actual IS DISTINCT FROM (SELECT fingerprint FROM public.apsp_schema_baseline WHERE singleton) THEN
        RAISE EXCEPTION 'existing schema differs from its recorded baseline';
    END IF;
END $$;

-- Creating the foreign keys takes SHARE ROW EXCLUSIVE on alicloud_accounts and
-- alicloud_resources, dropping their columns ACCESS EXCLUSIVE. Both are taken here, once and in
-- this order, so the script never upgrades a lock it holds.
LOCK TABLE public.alicloud_accounts, public.alicloud_resources IN ACCESS EXCLUSIVE MODE;

DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM public.alicloud_accounts WHERE access_key_id <> '' OR access_key_secret <> '') THEN
        RAISE EXCEPTION 'a cloud account still stores an inline access key; move it to the credential store first';
    END IF;
END $$;

CREATE TABLE public.cloud_account_billing (
    account_id uuid PRIMARY KEY REFERENCES public.alicloud_accounts(id),
    billing_enabled boolean DEFAULT false NOT NULL,
    balance_enabled boolean DEFAULT false NOT NULL,
    bill jsonb,
    bill_error text,
    traffic jsonb,
    traffic_error text,
    traffic_history_identity text,
    balance jsonb,
    balance_error text,
    balance_next_at bigint DEFAULT 0 NOT NULL
);

CREATE TABLE public.cloud_resource_billing (
    resource_id uuid PRIMARY KEY REFERENCES public.alicloud_resources(id),
    instance_bill jsonb,
    bill_error text,
    bill_next_at bigint DEFAULT 0 NOT NULL
);

INSERT INTO public.cloud_account_billing(account_id, billing_enabled, balance_enabled, bill, bill_error, traffic, traffic_error, traffic_history_identity, balance, balance_error, balance_next_at)
    SELECT id, billing_enabled, balance_enabled, bill, error_code, traffic, traffic_error, traffic_history_identity, balance, balance_error, balance_next_at
    FROM public.alicloud_accounts;
INSERT INTO public.cloud_resource_billing(resource_id, instance_bill, bill_error, bill_next_at)
    SELECT id, instance_bill, bill_error, bill_next_at FROM public.alicloud_resources;

DO $$
BEGIN
    IF (SELECT count(*) FROM public.cloud_account_billing) <> (SELECT count(*) FROM public.alicloud_accounts)
        OR EXISTS (
            SELECT 1 FROM public.alicloud_accounts a
            LEFT JOIN public.cloud_account_billing b ON b.account_id=a.id
            WHERE b.account_id IS NULL
                OR b.billing_enabled IS DISTINCT FROM a.billing_enabled
                OR b.balance_enabled IS DISTINCT FROM a.balance_enabled
                OR md5(b.bill::text) IS DISTINCT FROM md5(a.bill::text)
                OR b.bill_error IS DISTINCT FROM a.error_code
                OR md5(b.traffic::text) IS DISTINCT FROM md5(a.traffic::text)
                OR b.traffic_error IS DISTINCT FROM a.traffic_error
                OR b.traffic_history_identity IS DISTINCT FROM a.traffic_history_identity
                OR md5(b.balance::text) IS DISTINCT FROM md5(a.balance::text)
                OR b.balance_error IS DISTINCT FROM a.balance_error
                OR b.balance_next_at IS DISTINCT FROM a.balance_next_at)
    THEN RAISE EXCEPTION 'copied account billing differs from the account rows'; END IF;
    IF (SELECT count(*) FROM public.cloud_resource_billing) <> (SELECT count(*) FROM public.alicloud_resources)
        OR EXISTS (
            SELECT 1 FROM public.alicloud_resources r
            LEFT JOIN public.cloud_resource_billing b ON b.resource_id=r.id
            WHERE b.resource_id IS NULL
                OR md5(b.instance_bill::text) IS DISTINCT FROM md5(r.instance_bill::text)
                OR b.bill_error IS DISTINCT FROM r.bill_error
                OR b.bill_next_at IS DISTINCT FROM r.bill_next_at)
    THEN RAISE EXCEPTION 'copied instance bills differ from the resource rows'; END IF;
END $$;

ALTER TABLE public.alicloud_accounts
    DROP COLUMN access_key_id,
    DROP COLUMN access_key_secret,
    DROP COLUMN bill,
    DROP COLUMN traffic,
    DROP COLUMN traffic_error,
    DROP COLUMN error_code,
    DROP COLUMN balance,
    DROP COLUMN balance_error,
    DROP COLUMN balance_next_at,
    DROP COLUMN billing_enabled,
    DROP COLUMN balance_enabled,
    DROP COLUMN traffic_history_identity;

ALTER TABLE public.alicloud_resources
    DROP COLUMN instance_bill,
    DROP COLUMN bill_error,
    DROP COLUMN bill_next_at;

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
    IF actual <> '6a79e639ec7074be935b43cbbdb96aa4' THEN RAISE EXCEPTION 'result differs from fresh target baseline: %',actual; END IF;
    UPDATE public.apsp_schema_baseline SET fingerprint=actual WHERE singleton;
    UPDATE public._sqlx_migrations SET checksum=decode('4e5b878357c270ec78791d19bcc3ad13d9178abd7b339b49c3b4aa52cf3ae664c3af61d0bb27bbfd2490f0ba82a9c33c','hex') WHERE version=1;
END $$;
