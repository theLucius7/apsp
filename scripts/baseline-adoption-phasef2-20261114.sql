-- One-time baseline adoption, phase F step 2: from the baseline of phase F step 6 (checksum
-- 4e5b8783…, fingerprint 6a79e639…) to the one where each price and expiry fact has one place
-- (checksum 277a1788…, fingerprint 1f00c9b3…):
--   fleet_profiles.asset                     -> servers.asset_settings->'procurement': the
--                                               supplier, order and cloud account references,
--                                               region, note and purchased specification;
--                                               a price or expiry key in it must equal the
--                                               server's asset settings and is dropped
--   fleet_templates.config->'asset'          -> config->'asset_settings'->'procurement', on
--                                               the same terms against the template's settings
--   operations_cloud_links.purchase_reference,
--     purchase_amount, currency,
--     monthly_budget, expires_at            -> costs' cloud_resource_costs: every budget with
--                                               the link's currency as its budget_currency, and
--                                               the purchase (with that currency) of a resource
--                                               without a server. A linked resource's purchase
--                                               is its server's asset settings: it must be empty
--                                               or equal to them (an order reference the server
--                                               lacks becomes the server's)
-- Data the new model cannot hold without guessing (another key, a conflicting value, a
-- linked purchase the server does not record) stops the script before anything changes.
-- fleet_cost_records keeps its rows and columns; only the code that writes it moved to costs.
-- The old panel keeps running while this executes and nothing is rolled back afterwards, so
-- every table it changes is locked first: values written before the locks are granted are
-- converted, and none can be written after. Until the new image replaces it, the old panel's
-- statements that read the dropped columns fail and write nothing: the operations profile,
-- template enrolment, supplier comparison, cloud link form and cloud observation step stop for
-- those seconds and resume under the new image. Where the script gives a server a procurement
-- record, the old panel's AssetSettings (which refuses unknown keys) cannot read that server's
-- asset settings until the swap either, so /api/servers, alert evaluation and telemetry
-- persistence fail for it meanwhile; production has no profile asset, so no server gets one.
-- Fingerprints are PostgreSQL 16 values with sqlx's _sqlx_migrations present, checked on a
-- fresh initialisation of each baseline.
-- psql -v ON_ERROR_STOP=1 -1 -f scripts/baseline-adoption-phasef2-20261114.sql
SET LOCAL lock_timeout = '5s';
SET LOCAL search_path TO public;
SELECT pg_advisory_xact_lock(73401220);
DO $$
DECLARE actual text;
BEGIN
    IF (SELECT count(*) FROM public._sqlx_migrations) <> 1 OR NOT EXISTS (
        SELECT 1 FROM public._sqlx_migrations WHERE version=1 AND success AND checksum=decode('4e5b878357c270ec78791d19bcc3ad13d9178abd7b339b49c3b4aa52cf3ae664c3af61d0bb27bbfd2490f0ba82a9c33c','hex')
    ) THEN RAISE EXCEPTION 'expected the phase F step 6 baseline 4e5b8783; refusing to modify database'; END IF;
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
    IF actual <> '6a79e639ec7074be935b43cbbdb96aa4' OR actual IS DISTINCT FROM (SELECT fingerprint FROM public.apsp_schema_baseline WHERE singleton) THEN
        RAISE EXCEPTION 'existing schema differs from its recorded baseline';
    END IF;
END $$;

-- Dropping columns takes ACCESS EXCLUSIVE on fleet_profiles and operations_cloud_links;
-- writing the templates and servers' asset settings needs their rows to stay as read; creating
-- the foreign key takes SHARE ROW EXCLUSIVE on alicloud_resources. All are taken here, before
-- any work, once and in this order: the tables the old panel rarely touches first, servers
-- last, so the busy servers table is held only while the script works, never while it waits
-- for another lock, and no old-panel transaction that reads a profile, link or cloud resource
-- before it locks a server can wait for the script while the script waits for it. Plain reads
-- of servers, the templates and the cloud resources continue meanwhile.
LOCK TABLE public.fleet_profiles, public.operations_cloud_links IN ACCESS EXCLUSIVE MODE;
LOCK TABLE public.fleet_templates, public.alicloud_resources IN EXCLUSIVE MODE;
LOCK TABLE public.servers IN EXCLUSIVE MODE;

-- The procurement records to write: each profile's asset over its server's asset settings and
-- each template's asset over the template's. A price or expiry key must equal the settings'
-- value (the typed default when the settings never saved it); texts must be texts within the
-- record's limits and the purchased specification non-negative integers. Empty texts and
-- unknown values are left out, as the panel stores the record.
CREATE TEMPORARY TABLE adoption_procurement ON COMMIT DROP AS
SELECT 'server'::text AS kind, f.server_id::text AS id, f.asset, s.asset_settings AS settings, NULL::jsonb AS procurement
FROM public.fleet_profiles f JOIN public.servers s ON s.id=f.server_id
WHERE f.asset <> '{}'::jsonb
UNION ALL
SELECT 'template', t.id::text, t.config->'asset', COALESCE(t.config->'asset_settings','{}'::jsonb), NULL::jsonb
FROM public.fleet_templates t WHERE t.config ? 'asset';

DO $$
DECLARE r record; k text; v jsonb; merged jsonb; purchased jsonb; settings jsonb; entry text; checked text; maximum integer;
BEGIN
    IF EXISTS (SELECT 1 FROM public.servers WHERE asset_settings ? 'procurement')
        OR EXISTS (SELECT 1 FROM public.fleet_templates WHERE config->'asset_settings' ? 'procurement') THEN
        RAISE EXCEPTION 'a procurement record already exists in asset settings; refusing to merge another into it';
    END IF;
    FOR r IN SELECT * FROM adoption_procurement ORDER BY kind, id LOOP
        IF jsonb_typeof(r.asset) <> 'object' THEN
            RAISE EXCEPTION '% % has a profile asset that is not an object', r.kind, r.id;
        END IF;
        IF jsonb_typeof(r.settings) <> 'object' THEN
            RAISE EXCEPTION '% % has asset settings that are not an object', r.kind, r.id;
        END IF;
        settings := r.settings;
        merged := '{}'::jsonb;
        FOR k, v IN SELECT * FROM jsonb_each(r.asset) LOOP
            IF k IN ('provider','cloud_account_reference','region','order_reference','note') THEN
                IF jsonb_typeof(v) = 'string' THEN
                    entry := btrim(v #>> '{}');
                    maximum := CASE k WHEN 'provider' THEN 128 WHEN 'cloud_account_reference' THEN 256 WHEN 'region' THEN 64 WHEN 'order_reference' THEN 512 ELSE 4096 END;
                    checked := CASE WHEN k = 'note' THEN translate(entry, E'\n\t', '') ELSE entry END;
                    IF length(entry) > maximum OR checked ~ '[[:cntrl:]]' THEN
                        RAISE EXCEPTION '% % records a % the asset record cannot hold (too long or with control characters)', r.kind, r.id, k;
                    END IF;
                    IF entry <> '' THEN
                        merged := merged || jsonb_build_object(k, entry);
                    END IF;
                ELSIF jsonb_typeof(v) <> 'null' THEN
                    RAISE EXCEPTION '% % records % as %, not as text', r.kind, r.id, k, jsonb_typeof(v);
                END IF;
            ELSIF k = 'purchased' THEN
                IF jsonb_typeof(v) <> 'object' THEN
                    RAISE EXCEPTION '% % records the purchased specification as %', r.kind, r.id, jsonb_typeof(v);
                END IF;
                IF EXISTS (SELECT 1 FROM jsonb_each(v) e WHERE e.key NOT IN ('cpu_cores','memory_bytes','disk_bytes','bandwidth_mbps')
                        OR NOT (jsonb_typeof(e.value)='null' OR (jsonb_typeof(e.value)='number' AND (e.value #>> '{}') ~ '^[0-9]{1,20}$' AND (e.value #>> '{}')::numeric <= 18446744073709551615))) THEN
                    RAISE EXCEPTION '% % records a purchased specification the asset record cannot hold', r.kind, r.id;
                END IF;
                purchased := COALESCE((SELECT jsonb_object_agg(e.key, e.value) FROM jsonb_each(v) e WHERE jsonb_typeof(e.value)='number'), '{}'::jsonb);
                IF purchased <> '{}'::jsonb THEN
                    merged := merged || jsonb_build_object('purchased', purchased);
                END IF;
            ELSIF k = 'price' THEN
                IF NOT ((jsonb_typeof(v)='null' AND settings->>'price' IS NULL)
                        OR (jsonb_typeof(v) IN ('string','number') AND settings->>'price' IS NOT NULL
                            AND (v #>> '{}') ~ '^[0-9]+(\.[0-9]*)?$' AND (v #>> '{}')::numeric = (settings->>'price')::numeric)) THEN
                    RAISE EXCEPTION '% % records a price that differs from its asset settings', r.kind, r.id;
                END IF;
            ELSIF k = 'currency' THEN
                IF NOT (jsonb_typeof(v)='string' AND upper(btrim(v #>> '{}')) = COALESCE(settings->>'currency','CNY')) THEN
                    RAISE EXCEPTION '% % records a currency that differs from its asset settings', r.kind, r.id;
                END IF;
            ELSIF k = 'billing_cycle' THEN
                IF NOT (jsonb_typeof(v)='number' AND (v #>> '{}')::numeric = COALESCE((settings->>'billing_cycle')::numeric,30)) THEN
                    RAISE EXCEPTION '% % records a billing cycle that differs from its asset settings', r.kind, r.id;
                END IF;
            ELSIF k = 'expires_at' THEN
                IF NOT ((jsonb_typeof(v)='null' AND jsonb_typeof(settings->'expires_at') IS DISTINCT FROM 'number')
                        OR (jsonb_typeof(v)='number' AND jsonb_typeof(settings->'expires_at')='number' AND (v #>> '{}')::numeric = (settings->>'expires_at')::numeric)) THEN
                    RAISE EXCEPTION '% % records an expiry that differs from its asset settings', r.kind, r.id;
                END IF;
            ELSIF k = 'auto_renewal' THEN
                IF NOT (jsonb_typeof(v)='boolean' AND (v #>> '{}')::boolean = COALESCE((settings->>'auto_renewal')::boolean,false)) THEN
                    RAISE EXCEPTION '% % records an automatic renewal that differs from its asset settings', r.kind, r.id;
                END IF;
            ELSE
                RAISE EXCEPTION '% % records "%" in its profile asset, which the asset record cannot hold', r.kind, r.id, k;
            END IF;
        END LOOP;
        UPDATE adoption_procurement SET procurement=NULLIF(merged, '{}'::jsonb) WHERE kind=r.kind AND id=r.id;
        IF r.kind = 'server' THEN
            IF merged <> '{}'::jsonb THEN
                UPDATE public.servers SET asset_settings=asset_settings || jsonb_build_object('procurement', merged) WHERE id=r.id::bigint;
            END IF;
        ELSE
            UPDATE public.fleet_templates SET config=CASE WHEN merged = '{}'::jsonb THEN config-'asset'
                ELSE (config-'asset') || jsonb_build_object('asset_settings', COALESCE(config->'asset_settings','{}'::jsonb) || jsonb_build_object('procurement', merged)) END
            WHERE id=r.id::uuid;
        END IF;
    END LOOP;
    -- Every record went where it belongs, and nowhere else.
    IF EXISTS (SELECT 1 FROM public.servers s LEFT JOIN adoption_procurement p ON p.kind='server' AND p.id=s.id::text
            WHERE (s.asset_settings->'procurement') IS DISTINCT FROM p.procurement)
        OR EXISTS (SELECT 1 FROM public.fleet_templates t LEFT JOIN adoption_procurement p ON p.kind='template' AND p.id=t.id::text
            WHERE t.config ? 'asset' OR (t.config->'asset_settings'->'procurement') IS DISTINCT FROM p.procurement) THEN
        RAISE EXCEPTION 'merged procurement records differ from the profile and template assets';
    END IF;
END $$;

-- A linked resource's purchase must be its server's (after the merge above): empty, or the
-- server's price in its currency, its expiry and its order reference. An order reference the
-- server does not record becomes its own.
DO $$
DECLARE r record; reference text; existing text;
BEGIN
    FOR r IN SELECT l.resource_id, l.server_id, l.purchase_reference, l.purchase_amount, l.currency, l.expires_at, s.asset_settings AS settings
        FROM public.operations_cloud_links l JOIN public.servers s ON s.id=l.server_id
        WHERE l.purchase_reference <> '' OR l.purchase_amount IS NOT NULL OR l.expires_at IS NOT NULL
        ORDER BY l.server_id, l.resource_id LOOP
        IF r.purchase_amount IS NOT NULL AND NOT (r.settings->>'price' IS NOT NULL
                AND r.purchase_amount ~ '^[0-9]+(\.[0-9]*)?$' AND r.purchase_amount::numeric = (r.settings->>'price')::numeric
                AND r.currency IS NOT DISTINCT FROM COALESCE(r.settings->>'currency','CNY')) THEN
            RAISE EXCEPTION 'cloud resource % records a purchase amount that differs from the price of its server %', r.resource_id, r.server_id;
        END IF;
        IF r.expires_at IS NOT NULL AND NOT (jsonb_typeof(r.settings->'expires_at')='number' AND (r.settings->>'expires_at')::numeric = r.expires_at) THEN
            RAISE EXCEPTION 'cloud resource % records an expiry that differs from the expiry of its server %', r.resource_id, r.server_id;
        END IF;
        reference := btrim(r.purchase_reference);
        IF length(reference) > 512 OR reference ~ '[[:cntrl:]]' THEN
            RAISE EXCEPTION 'cloud resource % records a purchase reference that cannot be an order reference', r.resource_id;
        END IF;
        IF reference <> '' THEN
            -- Read again: an earlier link of the same server may have given it one.
            SELECT COALESCE(asset_settings->'procurement'->>'order_reference','') INTO existing FROM public.servers WHERE id=r.server_id;
            IF existing = '' THEN
                UPDATE public.servers SET asset_settings=asset_settings || jsonb_build_object('procurement', COALESCE(asset_settings->'procurement','{}'::jsonb) || jsonb_build_object('order_reference', reference))
                WHERE id=r.server_id;
            ELSIF existing <> reference THEN
                RAISE EXCEPTION 'cloud resource % records a purchase reference that differs from the order reference of its server %', r.resource_id, r.server_id;
            END IF;
        END IF;
    END LOOP;
    IF EXISTS (SELECT 1 FROM public.operations_cloud_links l JOIN public.servers s ON s.id=l.server_id
            WHERE btrim(l.purchase_reference) <> '' AND s.asset_settings->'procurement'->>'order_reference' IS DISTINCT FROM btrim(l.purchase_reference)) THEN
        RAISE EXCEPTION 'a linked purchase reference is not its server''s order reference';
    END IF;
END $$;

CREATE TABLE public.cloud_resource_costs (
    resource_id uuid PRIMARY KEY REFERENCES public.alicloud_resources(id),
    purchase_reference text DEFAULT ''::text NOT NULL,
    purchase_amount text,
    currency text,
    monthly_budget text,
    budget_currency text,
    expires_at bigint
);

-- Every link's budget, in the link's one currency; the purchase, with that currency, only of a
-- resource without a server.
INSERT INTO public.cloud_resource_costs(resource_id, purchase_reference, purchase_amount, currency, monthly_budget, budget_currency, expires_at)
    SELECT resource_id,
        CASE WHEN server_id IS NULL THEN purchase_reference ELSE '' END,
        CASE WHEN server_id IS NULL THEN purchase_amount END,
        CASE WHEN server_id IS NULL THEN currency END,
        monthly_budget,
        CASE WHEN monthly_budget IS NOT NULL THEN currency END,
        CASE WHEN server_id IS NULL THEN expires_at END
    FROM public.operations_cloud_links
    WHERE monthly_budget IS NOT NULL
        OR (server_id IS NULL AND (purchase_reference <> '' OR purchase_amount IS NOT NULL OR currency IS NOT NULL OR expires_at IS NOT NULL));

DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM public.operations_cloud_links l
        LEFT JOIN public.cloud_resource_costs c ON c.resource_id=l.resource_id
        WHERE (c.resource_id IS NULL) <> (l.monthly_budget IS NULL
                AND (l.server_id IS NOT NULL OR (l.purchase_reference='' AND l.purchase_amount IS NULL AND l.currency IS NULL AND l.expires_at IS NULL)))
            OR (c.resource_id IS NOT NULL AND (c.monthly_budget IS DISTINCT FROM l.monthly_budget
                OR c.budget_currency IS DISTINCT FROM CASE WHEN l.monthly_budget IS NOT NULL THEN l.currency END
                OR c.currency IS DISTINCT FROM CASE WHEN l.server_id IS NULL THEN l.currency END
                OR c.purchase_reference IS DISTINCT FROM CASE WHEN l.server_id IS NULL THEN l.purchase_reference ELSE '' END
                OR c.purchase_amount IS DISTINCT FROM CASE WHEN l.server_id IS NULL THEN l.purchase_amount END
                OR c.expires_at IS DISTINCT FROM CASE WHEN l.server_id IS NULL THEN l.expires_at END))
    ) THEN
        RAISE EXCEPTION 'copied cloud resource costs differ from the links';
    END IF;
END $$;

ALTER TABLE public.fleet_profiles DROP COLUMN asset;

ALTER TABLE public.operations_cloud_links
    DROP COLUMN purchase_reference,
    DROP COLUMN purchase_amount,
    DROP COLUMN currency,
    DROP COLUMN monthly_budget,
    DROP COLUMN expires_at;

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
    IF actual <> '1f00c9b3ee040ba307e844e003f5f15a' THEN RAISE EXCEPTION 'result differs from fresh target baseline: %',actual; END IF;
    UPDATE public.apsp_schema_baseline SET fingerprint=actual WHERE singleton;
    UPDATE public._sqlx_migrations SET checksum=decode('277a1788abae2b8d4b40b8fa89a5c6339d419129dd4e213c4bc6e4a21eeeca8ee9b5921e65da144eca9738b6f71b79f8','hex') WHERE version=1;
END $$;
