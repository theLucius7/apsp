-- One-time baseline adoption, phase F step 1: from the baseline that removed the commerce
-- tables (checksum a1c4b16e…, fingerprint bd3c95e0…) to the one that splits sessions and merges
-- the commerce events (checksum f9a10013…, fingerprint 5c70bc13…).
--   F0  The structure fingerprint counts a column by its position among the columns that
--       exist instead of by attnum, so a dropped column leaves no gap that a fresh
--       initialisation or the deployer's pg_dump copy could never reproduce. The source
--       fingerprint below is still checked the old way; the result is computed the new way.
--   F1  Device sessions move from sessions to device_sessions (resources); sessions keeps the
--       administrator sessions only.
--   F3  singbox_commerce_events joins singbox_operation_events (both access) and is dropped:
--       admin:N and administrator:N become administrator_id, other actors and the object id
--       go into detail, and the members' own account events take the security_portal_*
--       names (a password login is security_portal_login with authentication "password").
-- The old panel keeps running while this executes and nothing is rolled back afterwards, so
-- every copy and drop happens in this one locked transaction: rows written before the locks
-- are granted are moved, and none can be written after. Fingerprints are PostgreSQL 16 values
-- with sqlx's _sqlx_migrations present, checked on a fresh initialisation of each baseline.
-- psql -v ON_ERROR_STOP=1 -1 -f scripts/baseline-adoption-phasef1-20261114.sql
SET LOCAL lock_timeout = '5s';
SET LOCAL search_path TO public;
SELECT pg_advisory_xact_lock(73401220);
DO $$
DECLARE actual text;
BEGIN
    IF (SELECT count(*) FROM public._sqlx_migrations) <> 1 OR NOT EXISTS (
        SELECT 1 FROM public._sqlx_migrations WHERE version=1 AND success AND checksum=decode('a1c4b16eed22316dc8bbe6a80f3ab651267305bb3efce631ca621e4978be764d65f172727ae032abf3eb27b3c90fa416','hex')
    ) THEN RAISE EXCEPTION 'expected the commerce-removal baseline a1c4b16e; refusing to modify database'; END IF;
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

-- Dropping sessions_server_id_fkey and the commerce table's user key takes ACCESS EXCLUSIVE on
-- servers and users, where their triggers live. Every lock is taken here, in one order, so the
-- script never upgrades a lock it already holds.
LOCK TABLE public.servers, public.users, public.sessions, public.singbox_commerce_events IN ACCESS EXCLUSIVE MODE;
LOCK TABLE public.singbox_operation_events IN EXCLUSIVE MODE;

CREATE TEMPORARY TABLE adoption_before ON COMMIT DROP AS SELECT
    (SELECT count(*) FROM public.sessions WHERE server_id IS NOT NULL) AS device_sessions,
    (SELECT count(*) FROM public.sessions WHERE server_id IS NULL) AS administrator_sessions,
    (SELECT count(*) FROM public.singbox_commerce_events) AS commerce_events,
    (SELECT count(*) FROM public.singbox_operation_events) AS operation_events;

-- F1: device sessions get their own table; sessions keeps the administrator sessions.
CREATE TABLE public.device_sessions (
    token_hash text PRIMARY KEY,
    server_id bigint NOT NULL REFERENCES public.servers(id),
    expires_at bigint NOT NULL
);

CREATE INDEX device_sessions_server_idx ON public.device_sessions USING btree (server_id);

CREATE INDEX device_sessions_expiry_idx ON public.device_sessions USING btree (expires_at);

INSERT INTO public.device_sessions(token_hash, server_id, expires_at)
    SELECT token_hash, server_id, expires_at FROM public.sessions WHERE server_id IS NOT NULL;
DELETE FROM public.sessions WHERE server_id IS NOT NULL;
ALTER TABLE public.sessions DROP CONSTRAINT sessions_check, DROP CONSTRAINT sessions_server_id_fkey;
DROP INDEX public.sessions_server_idx;
ALTER TABLE public.sessions DROP COLUMN server_id, ALTER COLUMN admin_id SET NOT NULL;

-- F3: one event log for access. Actions the current code writes get the names it now uses;
-- the object id goes under the key its action names (dropped where it is the user's own id or
-- a fixed word), any other action keeps its name with the id under object_id.
INSERT INTO public.singbox_operation_events(administrator_id, user_id, action, detail, created_at)
SELECT CASE WHEN e.actor ~ '^(admin|administrator):[0-9]{1,18}$' THEN split_part(e.actor, ':', 2)::bigint END,
       e.user_id,
       COALESCE(m.action, e.action),
       COALESCE(m.detail, '{}'::jsonb)
           || CASE WHEN e.actor ~ '^(admin|administrator):[0-9]{1,18}$' THEN '{}'::jsonb ELSE jsonb_build_object('actor', e.actor) END
           || CASE WHEN m.source IS NOT NULL AND m.object_key IS NULL THEN '{}'::jsonb
                   ELSE jsonb_build_object(COALESCE(m.object_key, 'object_id'),
                       CASE WHEN e.object_id ~ '^[0-9]{1,18}$' THEN to_jsonb(e.object_id::bigint) ELSE to_jsonb(e.object_id) END) END
           || CASE WHEN jsonb_typeof(e.detail) = 'object' THEN e.detail ELSE jsonb_build_object('detail', e.detail) END,
       e.created_at
FROM public.singbox_commerce_events e
LEFT JOIN (VALUES
    ('password_login', 'security_portal_login', 'account_id', '{"authentication":"password","result":"verified"}'::jsonb),
    ('password_recovered', 'security_portal_password_recovery', NULL, NULL),
    ('password_changed', 'security_portal_password_change', NULL, NULL),
    ('email_changed', 'security_portal_email_change', NULL, NULL),
    ('invitation_claimed', 'invitation_claimed', 'invitation_id', NULL),
    ('invitation_rotated', 'invitation_rotated', 'invitation_id', NULL),
    ('invitation_purchased', 'invitation_purchased', 'invitation_id', NULL),
    ('invitation_settings_saved', 'invitation_settings_saved', NULL, NULL),
    ('mail_settings_changed', 'mail_settings_changed', NULL, NULL),
    ('settings_saved', 'settings_saved', NULL, NULL),
    ('content_saved', 'content_saved', 'content_id', NULL),
    ('content_deleted', 'content_deleted', 'content_id', NULL),
    ('channel_saved', 'channel_saved', 'channel_id', NULL),
    ('offer_saved', 'offer_saved', 'offer_id', NULL),
    ('duration_granted', 'duration_granted', 'assignment_id', NULL),
    ('nexus_imported', 'nexus_imported', 'import_id', NULL),
    ('nexus_compatibility_activated', 'nexus_compatibility_activated', 'import_id', NULL)
) AS m(source, action, object_key, detail) ON m.source = e.action
ORDER BY e.id;
DROP TABLE public.singbox_commerce_events;

DO $$
DECLARE actual text;
BEGIN
    IF (SELECT count(*) FROM public.device_sessions) <> (SELECT device_sessions FROM adoption_before)
        OR (SELECT count(*) FROM public.sessions) <> (SELECT administrator_sessions FROM adoption_before)
        OR (SELECT count(*) FROM public.singbox_operation_events) <> (SELECT operation_events + commerce_events FROM adoption_before)
    THEN RAISE EXCEPTION 'moved rows do not add up'; END IF;
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
    IF actual <> '5c70bc13eb7f87e4e8ffe072cbc2f754' THEN RAISE EXCEPTION 'result differs from fresh target baseline: %',actual; END IF;
    UPDATE public.apsp_schema_baseline SET fingerprint=actual WHERE singleton;
    UPDATE public._sqlx_migrations SET checksum=decode('f9a100135e5f9db2fc54a2720f57cf2bd6265ce2d7038796b1c7a1c5785f6f82c559526dcd40b54b0ce77bed21f25bca','hex') WHERE version=1;
END $$;
