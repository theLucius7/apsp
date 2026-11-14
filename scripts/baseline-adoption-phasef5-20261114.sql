-- One-time baseline adoption, phase F step 5: from the baseline of phase F step 4 (checksum
-- 51ebdbd2…, fingerprint 1ff76742…) to the one where the traffic service keeps its own state
-- about each server instead of writing it on the server row (checksum 345adb3b…,
-- fingerprint 09938605…):
--   servers.manifest_rev, dirty_at             -> runtime_publications
--   server_plugins (server_id, plugin, ...)     -> runtime_enablements (server_id, module, ...),
--                                                 the runtime 'sing-box' stored as its module
--                                                 identifier 'singbox' like every other module
--                                                 column; the console still shows "sing-box"
--   runtime_control_requests                    +  runtime_control_checkpoint_revision_idx, the
--                                                 checkpoint lookup by expected revision
-- Every server, deleted ones included, gets its publication row with the revision and the
-- pending mark copied exactly: deployments are keyed by server, module and revision, and an
-- Agent ignores a manifest whose revision is lower than one it has seen. Every enablement row
-- is copied with its module mapped. Each copy is checked row by row before the old columns
-- and the old table are dropped, all in this one transaction.
-- The old panel keeps running while this executes and nothing is rolled back afterwards, so
-- servers and server_plugins are locked exclusively first: values written before the locks
-- are granted are copied, and none can be written after. Until the new image replaces it, the
-- old panel's statements that name the dropped columns or server_plugins fail and write
-- nothing; its publication cycle publishes nothing, and Agents retry what it refuses.
-- Fingerprints are PostgreSQL 16 values with sqlx's _sqlx_migrations present, checked on a
-- fresh initialisation of each baseline.
-- psql -v ON_ERROR_STOP=1 -1 -f scripts/baseline-adoption-phasef5-20261114.sql
SET LOCAL lock_timeout = '5s';
SET LOCAL search_path TO public;
SELECT pg_advisory_xact_lock(73401220);
DO $$
DECLARE actual text;
BEGIN
    IF (SELECT count(*) FROM public._sqlx_migrations) <> 1 OR NOT EXISTS (
        SELECT 1 FROM public._sqlx_migrations WHERE version=1 AND success AND checksum=decode('51ebdbd2bb12a56bed2cef4ee29824deaa1cc024106129dae2d48b61265fae9d96afd57f8dc31ea20449e778495bfa2b','hex')
    ) THEN RAISE EXCEPTION 'expected the phase F step 4 baseline 51ebdbd2; refusing to modify database'; END IF;
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
    IF actual <> '1ff767422097a52e0c7430ad4ce4de7a' OR actual IS DISTINCT FROM (SELECT fingerprint FROM public.apsp_schema_baseline WHERE singleton) THEN
        RAISE EXCEPTION 'existing schema differs from its recorded baseline';
    END IF;
END $$;

-- Creating the foreign keys takes SHARE ROW EXCLUSIVE on servers, dropping the columns and the
-- old table ACCESS EXCLUSIVE on servers and server_plugins, and building the index SHARE on
-- runtime_control_requests. All are taken here, once and in this order, so the script never
-- upgrades a lock it holds.
LOCK TABLE public.servers, public.server_plugins IN ACCESS EXCLUSIVE MODE;
LOCK TABLE public.runtime_control_requests IN SHARE MODE;

CREATE TABLE public.runtime_publications (
    server_id bigint PRIMARY KEY REFERENCES public.servers(id) ON DELETE CASCADE,
    manifest_rev bigint DEFAULT 0 NOT NULL CHECK (manifest_rev >= 0),
    dirty_at bigint
);

CREATE TABLE public.runtime_enablements (
    server_id bigint NOT NULL,
    module text NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    source text NOT NULL,
    enabled_at bigint NOT NULL,
    CONSTRAINT runtime_enablements_source_check CHECK ((source = ANY (ARRAY['administrator'::text, 'agent_capability'::text, 'legacy_nodes'::text, 'legacy_deployments'::text])))
);

ALTER TABLE ONLY public.runtime_enablements
    ADD CONSTRAINT runtime_enablements_pkey PRIMARY KEY (server_id, module);

ALTER TABLE ONLY public.runtime_enablements
    ADD CONSTRAINT runtime_enablements_server_id_fkey FOREIGN KEY (server_id) REFERENCES public.servers(id);

CREATE INDEX runtime_control_checkpoint_revision_idx ON public.runtime_control_requests USING btree (server_id, (((request_json -> 'expected'::text) ->> 'revision'::text)::bigint)) WHERE (kind = 'checkpoint'::text);

INSERT INTO public.runtime_publications(server_id, manifest_rev, dirty_at)
    SELECT id, manifest_rev, dirty_at FROM public.servers;
INSERT INTO public.runtime_enablements(server_id, module, enabled, source, enabled_at)
    SELECT server_id, CASE plugin WHEN 'sing-box' THEN 'singbox' ELSE plugin END, enabled, source, enabled_at
    FROM public.server_plugins;

DO $$
BEGIN
    IF (SELECT count(*) FROM public.runtime_publications) <> (SELECT count(*) FROM public.servers)
        OR EXISTS (
            SELECT 1 FROM public.servers s
            LEFT JOIN public.runtime_publications p ON p.server_id=s.id
            WHERE p.server_id IS NULL
                OR p.manifest_rev IS DISTINCT FROM s.manifest_rev
                OR p.dirty_at IS DISTINCT FROM s.dirty_at)
    THEN RAISE EXCEPTION 'copied publication state differs from the server rows'; END IF;
    IF (SELECT count(*) FROM public.runtime_enablements) <> (SELECT count(*) FROM public.server_plugins)
        OR EXISTS (
            SELECT 1 FROM public.server_plugins o
            LEFT JOIN public.runtime_enablements e ON e.server_id=o.server_id
                AND e.module=CASE o.plugin WHEN 'sing-box' THEN 'singbox' ELSE o.plugin END
            WHERE e.server_id IS NULL
                OR e.enabled IS DISTINCT FROM o.enabled
                OR e.source IS DISTINCT FROM o.source
                OR e.enabled_at IS DISTINCT FROM o.enabled_at)
    THEN RAISE EXCEPTION 'copied enablements differ from server_plugins'; END IF;
END $$;

ALTER TABLE public.servers
    DROP COLUMN manifest_rev,
    DROP COLUMN dirty_at;

DROP TABLE public.server_plugins;

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
    IF actual <> '099386056a68204d754deec1fa50340e' THEN RAISE EXCEPTION 'result differs from fresh target baseline: %',actual; END IF;
    UPDATE public.apsp_schema_baseline SET fingerprint=actual WHERE singleton;
    UPDATE public._sqlx_migrations SET checksum=decode('345adb3b4c0fd71c74fbc90f59b675ea10847936c20f764e7f471af9dff8787964a4d882e4eb83b44cb638d313653aed','hex') WHERE version=1;
END $$;
