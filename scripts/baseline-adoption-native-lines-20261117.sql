-- One-time baseline adoption: from the baseline of phase F step 7 (checksum edd39c0a…,
-- fingerprint ab219b8e…) to the one where migrated (preserved Nexus) lines serve every member of
-- their policy groups, ADR 0138 (checksum e96786fb…, fingerprint 17b172b9…):
--   singbox_nexus_runtime_retired_rates  new, empty: the rate of a preserved line once retired
--   singbox_legacy_served_users   new: the users the compatibility bridge serves with their
--                                 original client identity (they have one, or are non-staged
--                                 members while the bridge runs); singbox_legacy_served asks it
--                                 for one user
--   singbox_desired_accesses      a preserved line is left out only for those users; everyone
--                                 else gets ordinary credentials on it from their policy groups
--   singbox_nexus_guard_access    refuses credentials on a preserved line only for those users
--                                 and always lets credentials be deleted
--   singbox_usage_charged         new: cycle usage weighted by each line's rate, kept after
--                                 retirement
--   singbox_entitlements          users the bridge does not serve are charged at the lines'
--                                 rates; everyone else keeps the computation they had
--   singbox_runtime_credentials   new: ordinary credentials on preserved lines, for the hosts'
--                                 membership digest
-- The view is replaced before the new table is created: the table's foreign key takes SHARE ROW
-- EXCLUSIVE on nodes, and a panel transaction that reads the view and then writes nodes must not
-- find the script holding nodes while it waits for the view. Definitions that need the table
-- (the charged usage and the entitlements using it) come after it.
-- One new empty table and new definitions; no row is touched. Production has no credentials on
-- preserved lines and no user outside the bridge in a group holding one, so every desired,
-- eligible and entitled row, every quota figure and every host's membership stays the same.
-- Until the new image replaces the old panel, the old code reads the new definitions: they give
-- it the same rows as before for every user it can meet (see above).
-- Fingerprints are PostgreSQL 16 values with sqlx's _sqlx_migrations present, checked on a
-- fresh initialisation of each baseline.
-- psql -v ON_ERROR_STOP=1 -1 -f scripts/baseline-adoption-native-lines-20261117.sql
SET LOCAL lock_timeout = '5s';
SET LOCAL search_path TO public;
SELECT pg_advisory_xact_lock(73401220);
DO $$
DECLARE actual text;
BEGIN
    IF (SELECT count(*) FROM public._sqlx_migrations) <> 1 OR NOT EXISTS (
        SELECT 1 FROM public._sqlx_migrations WHERE version=1 AND success AND checksum=decode('edd39c0a103b64646b4602b406b499f83220ffa4dd2a8929ad14bd60345a94a2e9eaec4d7421383885d0898dd645a25c','hex')
    ) THEN RAISE EXCEPTION 'expected the phase F step 7 baseline edd39c0a; refusing to modify database'; END IF;
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
    IF actual <> 'ab219b8eacbbc3899de0b9428addfdbc' OR actual IS DISTINCT FROM (SELECT fingerprint FROM public.apsp_schema_baseline WHERE singleton) THEN
        RAISE EXCEPTION 'existing schema differs from its recorded baseline';
    END IF;
END $$;

CREATE FUNCTION public.singbox_legacy_served_users() RETURNS TABLE(user_id bigint)
    LANGUAGE sql STABLE
    AS $$
    -- The users the compatibility bridge serves on preserved lines with their original client
    -- identity: they have one, or the bridge issues it at its next exchange (the same members
    -- singbox_nexus_bridge_exchange enrols). Everyone else holds ordinary credentials on those
    -- lines (ADR 0138). A set that queries join once, not a test per row.
    SELECT i.user_id FROM singbox_nexus_identities i
    UNION
    SELECT m.user_id FROM singbox_members m
    WHERE m.state<>'staged' AND EXISTS(SELECT 1 FROM singbox_nexus_bridge b WHERE b.enabled)
$$;

CREATE FUNCTION public.singbox_legacy_served(member bigint) RETURNS boolean
    LANGUAGE sql STABLE
    AS $$
    -- Whether singbox_legacy_served_users holds this user.
    SELECT EXISTS(SELECT 1 FROM singbox_legacy_served_users() s WHERE s.user_id=member)
$$;

CREATE OR REPLACE FUNCTION public.singbox_nexus_guard_access() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    -- Users the bridge serves keep their original client identity on preserved lines.
    -- Removal is always allowed, so a user who becomes one loses ordinary credentials.
    IF TG_OP='DELETE' THEN RETURN OLD; END IF;
    IF EXISTS(SELECT 1 FROM singbox_nexus_runtime_nodes WHERE node_id=NEW.node_id)
        AND singbox_legacy_served(NEW.user_id) THEN
        RAISE EXCEPTION 'Nexus runtime lines keep the original client identities of bridge members; grant them through policy groups';
    END IF;
    RETURN NEW;
END $$;

CREATE OR REPLACE VIEW public.singbox_desired_accesses AS
 SELECT DISTINCT granted.user_id,
    granted.node_id
   FROM ((((( SELECT u_1.user_id,
            n_1.node_id
           FROM ((public.singbox_user_policies u_1
             JOIN public.singbox_policy_nodes n_1 ON ((n_1.group_id = u_1.group_id)))
             LEFT JOIN public.singbox_nexus_runtime_nodes r ON ((r.node_id = n_1.node_id)))
          WHERE ((NOT (EXISTS ( SELECT 1
                   FROM public.singbox_chains c_1
                  WHERE ((c_1.path_kind = 'ordered'::text) AND (c_1.entry_node_id = n_1.node_id) AND ((c_1.deleted_at IS NULL) OR (c_1.phase <> 'retired'::text)))))) AND ((r.node_id IS NULL) OR (NOT (u_1.user_id IN ( SELECT served.user_id
                   FROM public.singbox_legacy_served_users() served(user_id))))))
        UNION
         SELECT u_1.user_id,
            c_1.entry_node_id
           FROM ((public.singbox_user_policies u_1
             JOIN public.singbox_policy_chains p ON ((p.group_id = u_1.group_id)))
             JOIN public.singbox_chains c_1 ON ((c_1.id = p.chain_id)))
          WHERE (c_1.deleted_at IS NULL)) granted
     JOIN public.users u ON (((u.id = granted.user_id) AND (u.deleted_at IS NULL))))
     JOIN public.nodes n ON (((n.id = granted.node_id) AND (n.deleted_at IS NULL))))
     JOIN public.servers s ON (((s.id = n.server_id) AND (s.deleted_at IS NULL))))
     LEFT JOIN public.singbox_chains c ON (((c.entry_node_id = n.id) AND ((c.deleted_at IS NULL) OR ((c.path_kind = 'ordered'::text) AND (c.phase <> 'retired'::text))))))
  WHERE ((c.id IS NULL) OR ((c.deleted_at IS NULL) AND (n.protocol = 'vless-reality'::text) AND (EXISTS ( SELECT 1
           FROM public.singbox_ordered_chain_versions v
          WHERE ((v.chain_id = c.id) AND (v.generation = c.desired_generation)))) AND (NOT (EXISTS ( SELECT 1
           FROM ((public.singbox_ordered_chain_hops h
             LEFT JOIN public.nodes e ON ((e.id = h.managed_node_id)))
             LEFT JOIN public.servers es ON ((es.id = h.managed_server_id)))
          WHERE ((h.chain_id = c.id) AND (h.generation = c.desired_generation) AND (h.kind = 'managed'::text) AND ((e.id IS NULL) OR (es.id IS NULL) OR (e.deleted_at IS NOT NULL) OR (es.deleted_at IS NOT NULL) OR (NOT e.enabled))))))));

CREATE FUNCTION public.singbox_runtime_credentials() RETURNS TABLE(server_id bigint, node_id bigint, user_id bigint, uuid uuid)
    LANGUAGE sql STABLE
    AS $$
    -- The ordinary credentials on preserved lines that singbox_desired_accesses grants, for the
    -- hosts' membership digest (ADR 0138). It starts from the credentials on those lines, which
    -- are few, and checks only their grants.
    SELECT r.server_id,a.node_id,a.user_id,a.uuid FROM accesses a
    JOIN singbox_nexus_runtime_nodes r ON r.node_id=a.node_id
    JOIN nodes n ON n.id=a.node_id AND n.enabled AND n.deleted_at IS NULL
    JOIN servers s ON s.id=n.server_id AND s.deleted_at IS NULL
    JOIN users u ON u.id=a.user_id AND u.deleted_at IS NULL
    WHERE NOT EXISTS(SELECT 1 FROM singbox_legacy_served_users() served WHERE served.user_id=a.user_id)
        AND EXISTS(SELECT 1 FROM singbox_user_policies up JOIN singbox_policy_nodes pn ON pn.group_id=up.group_id
            WHERE up.user_id=a.user_id AND pn.node_id=a.node_id)
$$;

CREATE TABLE public.singbox_nexus_runtime_retired_rates (
    node_id bigint NOT NULL,
    rate numeric(12,6) NOT NULL,
    CONSTRAINT singbox_nexus_runtime_retired_rates_rate_check CHECK ((rate >= (0)::numeric))
);

ALTER TABLE ONLY public.singbox_nexus_runtime_retired_rates
    ADD CONSTRAINT singbox_nexus_runtime_retired_rates_pkey PRIMARY KEY (node_id);

ALTER TABLE ONLY public.singbox_nexus_runtime_retired_rates
    ADD CONSTRAINT singbox_nexus_runtime_retired_rates_node_id_fkey FOREIGN KEY (node_id) REFERENCES public.nodes(id);

CREATE FUNCTION public.singbox_usage_charged(p_user bigint, p_after bigint, p_until bigint) RETURNS numeric
    LANGUAGE sql STABLE STRICT
    AS $$
    -- The quota usage inside (p_after, p_until] of a user the bridge does not serve:
    -- singbox_usage_window with each line's traffic weighted by its rate. A preserved line keeps
    -- its original node's rate, also once retired; every other line counts once (ADR 0138).
    -- Whole bytes, rounded down.
    WITH bounds AS (
        SELECT (ceil((p_after + 1)::NUMERIC / 86400) * 86400)::BIGINT AS lo,
            (floor((p_until + 1)::NUMERIC / 86400) * 86400)::BIGINT AS hi
    )
    SELECT floor(CASE
        WHEN p_until <= p_after THEN 0::NUMERIC
        WHEN p_after < 0 THEN COALESCE((SELECT SUM((r.uplink + r.downlink) * COALESCE(x.rate, y.rate, 1)) FROM usage_records r
            LEFT JOIN singbox_nexus_runtime_nodes x ON x.node_id = r.node_id
            LEFT JOIN singbox_nexus_runtime_retired_rates y ON y.node_id = r.node_id
            WHERE r.user_id = p_user AND r.period_end > p_after AND r.period_end <= p_until), 0)
        ELSE COALESCE((SELECT SUM((d.uplink + d.downlink) * COALESCE(x.rate, y.rate, 1)) FROM singbox_usage_daily d
                LEFT JOIN singbox_nexus_runtime_nodes x ON x.node_id = d.node_id
                LEFT JOIN singbox_nexus_runtime_retired_rates y ON y.node_id = d.node_id
                WHERE d.user_id = p_user AND d.day >= b.lo AND d.day < b.hi), 0)
            + COALESCE((SELECT SUM((r.uplink + r.downlink) * COALESCE(x.rate, y.rate, 1)) FROM usage_records r
                LEFT JOIN singbox_nexus_runtime_nodes x ON x.node_id = r.node_id
                LEFT JOIN singbox_nexus_runtime_retired_rates y ON y.node_id = r.node_id
                WHERE r.user_id = p_user AND r.period_end > p_after
                    AND r.period_end < LEAST(b.lo, p_until + 1)), 0)
            + COALESCE((SELECT SUM((r.uplink + r.downlink) * COALESCE(x.rate, y.rate, 1)) FROM usage_records r
                LEFT JOIN singbox_nexus_runtime_nodes x ON x.node_id = r.node_id
                LEFT JOIN singbox_nexus_runtime_retired_rates y ON y.node_id = r.node_id
                WHERE r.user_id = p_user AND r.period_end >= GREATEST(b.lo, b.hi)
                    AND r.period_end <= p_until), 0)
        END)
    FROM bounds b
$$;

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
        LEFT JOIN LATERAL (SELECT CASE WHEN meter.user_id IS NOT NULL THEN meter.upload+meter.download+GREATEST(COALESCE(singbox_usage_window(u.id,b.cycle_start,b.next_reset),0)-COALESCE((SELECT SUM(d.uplink+d.downlink) FROM singbox_nexus_usage_deliveries d WHERE d.user_id=u.id AND d.period_end>b.cycle_start AND d.period_end<=b.next_reset),0),0) WHEN u.id IN (SELECT served.user_id FROM singbox_legacy_served_users() served) THEN singbox_usage_window(u.id,b.cycle_start,b.next_reset) ELSE singbox_usage_charged(u.id,b.cycle_start,b.next_reset) END AS used) t ON TRUE
        LEFT JOIN LATERAL (SELECT SUM(q.bytes) AS credit FROM singbox_quota_credits q WHERE q.user_id=u.id AND q.cycle_start=b.cycle_start AND q.next_reset=b.next_reset) c ON TRUE
        WHERE u.deleted_at IS NULL
    )
    SELECT id,package_group_id,package_name,monthly_bytes::TEXT,reset_day,reset_hour,reset_minute,timezone,starts_at,NULLIF(expires_at,253402300799),cycle_start,next_reset,used::TEXT,status,status IN('unmetered','active') FROM evaluated;
$$;

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
    IF actual <> '17b172b9687014f3d65dca438942d14f' THEN RAISE EXCEPTION 'result differs from fresh target baseline: %',actual; END IF;
    UPDATE public.apsp_schema_baseline SET fingerprint=actual WHERE singleton;
    UPDATE public._sqlx_migrations SET checksum=decode('e96786fbe65f76d8a161f463efb55c99f3c64bb13fb2a88f4263376ea5dc3b0ee7341820b2f407c91892e098f3fdfe81','hex') WHERE version=1;
END $$;
