-- Adopt the current baseline for native subscriptions and usage anchors.
-- No member is switched by this script. Each user is previewed and adopted separately.
-- Existing identity, billing and raw usage rows are retained.
-- Run with psql -X -v ON_ERROR_STOP=1 -1 -f this-file.sql.
SET LOCAL lock_timeout='5s';
SET LOCAL search_path TO public;
SELECT pg_advisory_xact_lock(73401220);
DO $$
DECLARE actual text;
BEGIN
    IF (SELECT COUNT(*) FROM public._sqlx_migrations)<>1 OR NOT EXISTS(
        SELECT 1 FROM public._sqlx_migrations WHERE version=1 AND success
        AND checksum=decode('e96786fbe65f76d8a161f463efb55c99f3c64bb13fb2a88f4263376ea5dc3b0ee7341820b2f407c91892e098f3fdfe81','hex')
    ) THEN RAISE EXCEPTION 'expected native-lines baseline e96786fb; refusing adoption'; END IF;
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
    IF actual<>'17b172b9687014f3d65dca438942d14f' OR NOT EXISTS(
        SELECT 1 FROM public.apsp_schema_baseline WHERE singleton AND fingerprint=actual
    ) THEN RAISE EXCEPTION 'unexpected source structure: %',actual; END IF;
END $$;

-- Keep column positions identical to 0001_current.sql (native_control_ready precedes
-- last_sync_at and activated_at in schema_fingerprint's row_number() ordering).
LOCK TABLE public.singbox_nexus_bridge IN ACCESS EXCLUSIVE MODE;
ALTER TABLE public.singbox_nexus_bridge RENAME COLUMN last_sync_at TO _adopt_last_sync_at;
ALTER TABLE public.singbox_nexus_bridge RENAME COLUMN activated_at TO _adopt_activated_at;
ALTER TABLE public.singbox_nexus_bridge
    ADD COLUMN native_control_ready boolean DEFAULT false NOT NULL,
    ADD COLUMN last_sync_at bigint,
    ADD COLUMN activated_at bigint;
UPDATE public.singbox_nexus_bridge
    SET last_sync_at = _adopt_last_sync_at,
        activated_at = _adopt_activated_at;
DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM public.singbox_nexus_bridge
        WHERE last_sync_at IS DISTINCT FROM _adopt_last_sync_at
           OR activated_at IS DISTINCT FROM _adopt_activated_at
    ) THEN
        RAISE EXCEPTION 'singbox_nexus_bridge column copy mismatch';
    END IF;
END $$;
ALTER TABLE public.singbox_nexus_bridge
    ALTER COLUMN activated_at SET NOT NULL,
    DROP COLUMN _adopt_last_sync_at,
    DROP COLUMN _adopt_activated_at;

CREATE TABLE public.singbox_native_adoptions (
    user_id bigint PRIMARY KEY REFERENCES public.users(id),
    assignment_id bigint REFERENCES public.singbox_package_assignments(id),
    adopted_at bigint NOT NULL,
    cycle_start bigint,
    next_reset bigint,
    used_bytes numeric(30,0) NOT NULL CHECK (used_bytes>=0),
    ledger_bytes numeric(30,0) NOT NULL CHECK (ledger_bytes>=0),
    legacy_retired_generation uuid,
    legacy_retired_at bigint,
    preview_digest text NOT NULL CHECK (preview_digest ~ '^[0-9a-f]{64}$'),
    CONSTRAINT singbox_native_adoptions_cycle CHECK ((cycle_start IS NULL AND next_reset IS NULL) OR (cycle_start IS NOT NULL AND next_reset IS NOT NULL AND cycle_start<=adopted_at AND next_reset>cycle_start))
);

CREATE OR REPLACE FUNCTION public.singbox_entitlements(at_s bigint) RETURNS TABLE(user_id bigint, package_group_id bigint, package_name text, monthly_bytes text, reset_day integer, reset_hour integer, reset_minute integer, timezone text, starts_at bigint, expires_at bigint, cycle_start bigint, next_reset bigint, used_bytes text, status text, allowed boolean)
    LANGUAGE sql STABLE
    AS $$
    WITH evaluated AS (
        SELECT u.id,a.package_group_id,a.package_name,a.monthly_bytes,a.reset_day,a.reset_hour,a.reset_minute,a.timezone,a.starts_at,a.expires_at,
            b.cycle_start,b.next_reset,GREATEST(COALESCE(t.used,0)-COALESCE(c.credit,0),0) AS used,
            CASE WHEN EXISTS(SELECT 1 FROM singbox_user_controls control WHERE control.user_id=u.id AND control.paused) THEN 'paused' WHEN m.state IS NOT NULL AND (m.state<>'active' OR (m.sponsor_required AND m.first_purchase_at IS NULL)) THEN CASE WHEN m.state='active' THEN 'pending' ELSE m.state END
                WHEN a.id IS NULL AND (bridge.enabled OR adopted.user_id IS NOT NULL) THEN 'no_package' WHEN a.id IS NULL THEN 'unmetered' WHEN at_s<a.starts_at THEN 'not_started' WHEN at_s>=a.expires_at THEN 'expired'
                WHEN a.monthly_bytes IS NOT NULL AND GREATEST(COALESCE(t.used,0)-COALESCE(c.credit,0),0)>=a.monthly_bytes THEN 'exhausted' ELSE 'active' END AS status
        FROM users u LEFT JOIN singbox_members m ON m.user_id=u.id LEFT JOIN singbox_user_packages p ON p.user_id=u.id
        LEFT JOIN singbox_package_assignments a ON a.id=p.assignment_id
        LEFT JOIN singbox_nexus_identities identity ON identity.user_id=u.id
        LEFT JOIN singbox_native_adoptions adopted ON adopted.user_id=u.id
        LEFT JOIN singbox_nexus_bridge bridge ON bridge.import_id=identity.import_id AND bridge.enabled AND adopted.user_id IS NULL
        LEFT JOIN singbox_nexus_metering meter ON meter.user_id=u.id AND bridge.singleton
        LEFT JOIN LATERAL singbox_cycle_bounds(at_s,a.reset_day,a.reset_hour,a.reset_minute,a.timezone) native ON TRUE
        LEFT JOIN LATERAL (SELECT adopted.user_id IS NOT NULL AND adopted.assignment_id IS NOT DISTINCT FROM a.id AND at_s>=adopted.adopted_at AND (adopted.next_reset IS NULL OR a.id IS NULL OR at_s<adopted.next_reset OR at_s>=a.expires_at) AS anchored) anchor ON TRUE
        LEFT JOIN LATERAL (SELECT CASE WHEN anchor.anchored THEN adopted.cycle_start WHEN meter.user_id IS NOT NULL THEN COALESCE(meter.last_reset_at,0) ELSE native.cycle_start END AS cycle_start,CASE WHEN anchor.anchored THEN adopted.next_reset WHEN meter.user_id IS NOT NULL THEN COALESCE(meter.next_reset_at,253402300799) ELSE native.next_reset END AS next_reset) b ON TRUE
        LEFT JOIN LATERAL (SELECT CASE WHEN anchor.anchored THEN adopted.used_bytes+GREATEST(COALESCE(singbox_usage_charged(u.id,b.cycle_start,b.next_reset),0)-adopted.ledger_bytes,0) WHEN meter.user_id IS NOT NULL THEN meter.upload+meter.download+GREATEST(COALESCE(singbox_usage_window(u.id,b.cycle_start,b.next_reset),0)-COALESCE((SELECT SUM(d.uplink+d.downlink) FROM singbox_nexus_usage_deliveries d WHERE d.user_id=u.id AND d.period_end>b.cycle_start AND d.period_end<=b.next_reset),0),0) WHEN u.id IN (SELECT served.user_id FROM singbox_legacy_served_users() served) THEN singbox_usage_window(u.id,b.cycle_start,b.next_reset) ELSE singbox_usage_charged(u.id,b.cycle_start,b.next_reset) END AS used) t ON TRUE
        LEFT JOIN LATERAL (SELECT SUM(q.bytes) AS credit FROM singbox_quota_credits q WHERE q.user_id=u.id AND q.cycle_start=b.cycle_start AND q.next_reset=b.next_reset) c ON TRUE
        WHERE u.deleted_at IS NULL
    )
    SELECT id,package_group_id,package_name,monthly_bytes::TEXT,reset_day,reset_hour,reset_minute,timezone,starts_at,NULLIF(expires_at,253402300799),cycle_start,next_reset,used::TEXT,status,status IN('unmetered','active') FROM evaluated;
$$;

CREATE OR REPLACE FUNCTION public.singbox_legacy_served_users() RETURNS TABLE(user_id bigint)
    LANGUAGE sql STABLE
    AS $$
    -- The users the compatibility bridge serves on preserved lines with their original client
    -- identity: they have one, or the bridge issues it at its next exchange (the same members
    -- singbox_nexus_bridge_exchange enrols). Everyone else holds ordinary credentials on those
    -- lines (ADR 0138). A set that queries join once, not a test per row.
    SELECT i.user_id FROM singbox_nexus_identities i WHERE NOT EXISTS(SELECT 1 FROM singbox_native_adoptions a WHERE a.user_id=i.user_id)
    UNION
    SELECT m.user_id FROM singbox_members m
    WHERE NOT EXISTS(SELECT 1 FROM singbox_native_adoptions a WHERE a.user_id=m.user_id) AND m.state<>'staged' AND EXISTS(SELECT 1 FROM singbox_nexus_bridge b WHERE b.enabled)
$$;

CREATE OR REPLACE FUNCTION public.singbox_nexus_bridge_begin(expected_import uuid) RETURNS jsonb
    LANGUAGE plpgsql
    AS $$
BEGIN
    PERFORM pg_advisory_xact_lock(73201001);
    UPDATE singbox_nexus_bridge SET pending_generation=COALESCE(pending_generation,gen_random_uuid()),native_control_ready=TRUE
        WHERE singleton AND enabled AND import_id=expected_import;
    IF NOT FOUND THEN RAISE EXCEPTION 'Compatibility bridge is not active for this import'; END IF;
    RETURN jsonb_build_object('started',TRUE);
END $$;

CREATE OR REPLACE FUNCTION public.singbox_nexus_bridge_exchange(sample jsonb) RETURNS jsonb
    LANGUAGE plpgsql
    AS $_$
DECLARE bridge singbox_nexus_bridge%ROWTYPE; generation UUID; observed BIGINT; entry JSONB; result JSONB;
BEGIN
    PERFORM pg_advisory_xact_lock(73201001);
    SELECT * INTO STRICT bridge FROM singbox_nexus_bridge WHERE singleton AND enabled FOR UPDATE;
    IF sample->>'import_id' IS DISTINCT FROM bridge.import_id::text OR jsonb_typeof(sample->'members') IS DISTINCT FROM 'array' THEN
        RAISE EXCEPTION 'Invalid compatibility sample';
    END IF;
    IF jsonb_array_length(sample->'members')>20000 OR sample->>'observed_at' IS NULL THEN
        RAISE EXCEPTION 'Invalid compatibility sample';
    END IF;
    observed=(sample->>'observed_at')::BIGINT;
    IF observed<EXTRACT(EPOCH FROM now())::BIGINT-120 OR observed>EXTRACT(EPOCH FROM now())::BIGINT+30 THEN RAISE EXCEPTION 'Expired compatibility sample'; END IF;
    FOR entry IN SELECT value FROM jsonb_array_elements(sample->'members') LOOP
        IF COALESCE(entry->>'id','')!~'^[1-9][0-9]{0,15}$' OR COALESCE(entry->>'upload','')!~'^[0-9]{1,20}$' OR COALESCE(entry->>'download','')!~'^[0-9]{1,20}$' THEN RAISE EXCEPTION 'Invalid usage counter'; END IF;
        IF EXISTS(SELECT 1 FROM singbox_nexus_identities WHERE user_id=(entry->>'id')::BIGINT AND import_id=bridge.import_id) AND NOT EXISTS(SELECT 1 FROM singbox_native_adoptions WHERE user_id=(entry->>'id')::BIGINT) THEN
            INSERT INTO singbox_nexus_metering(user_id,upload,download,last_reset_at,next_reset_at,observed_at)
            VALUES((entry->>'id')::BIGINT,(entry->>'upload')::NUMERIC,(entry->>'download')::NUMERIC,(entry->>'last_reset_at')::BIGINT,(entry->>'next_reset_at')::BIGINT,observed)
            ON CONFLICT(user_id) DO UPDATE SET upload=EXCLUDED.upload,download=EXCLUDED.download,last_reset_at=EXCLUDED.last_reset_at,next_reset_at=EXCLUDED.next_reset_at,observed_at=EXCLUDED.observed_at
            WHERE EXCLUDED.observed_at>=singbox_nexus_metering.observed_at;
        END IF;
    END LOOP;
    INSERT INTO singbox_nexus_identities(user_id,import_id,legacy_uuid,original_state,original_plan_id,quota_bytes,used_upload,used_download,expires_at)
        SELECT m.user_id,bridge.import_id,gen_random_uuid(),m.state,p.legacy_plan_id,a.monthly_bytes,0,0,a.expires_at
        FROM singbox_members m JOIN users u ON u.id=m.user_id
        LEFT JOIN singbox_user_packages up ON up.user_id=m.user_id
        LEFT JOIN singbox_package_assignments a ON a.id=up.assignment_id
        LEFT JOIN singbox_nexus_plan_mappings p ON p.package_group_id=a.package_group_id AND p.import_id=bridge.import_id
        WHERE m.state<>'staged' AND u.deleted_at IS NULL AND NOT EXISTS(SELECT 1 FROM singbox_native_adoptions adopted WHERE adopted.user_id=m.user_id) ON CONFLICT(user_id) DO NOTHING;
    IF EXISTS(SELECT 1 FROM singbox_members m JOIN singbox_user_packages up ON up.user_id=m.user_id JOIN singbox_package_assignments a ON a.id=up.assignment_id
        JOIN singbox_nexus_identities i ON i.user_id=m.user_id LEFT JOIN singbox_nexus_plan_mappings p ON p.package_group_id=a.package_group_id
        WHERE i.import_id=bridge.import_id AND NOT EXISTS(SELECT 1 FROM singbox_native_adoptions adopted WHERE adopted.user_id=i.user_id) AND m.state='active' AND (p.package_group_id IS NULL OR p.legacy_group_id IS NULL)) THEN
        RAISE EXCEPTION 'Active member package has no compatibility mapping';
    END IF;
    SELECT COALESCE(jsonb_agg(jsonb_build_object(
        'id',u.id,'email',COALESCE(m.email,u.id::text||'@member.invalid'),'uuid',i.legacy_uuid,'subscription_token',u.subscription_token,
        'balance',w.balance_cents,'banned',CASE WHEN u.deleted_at IS NOT NULL OR m.state<>'active' OR (m.sponsor_required AND m.first_purchase_at IS NULL) OR a.id IS NULL THEN 1 ELSE 0 END,
        'member_status',CASE WHEN u.deleted_at IS NOT NULL OR m.state='archived' THEN 'ARCHIVED' WHEN m.state='suspended' THEN 'SUSPENDED' ELSE 'ACTIVE' END,
        'plan_id',p.legacy_plan_id,'group_id',singbox_member_legacy_group(u.id),
        'quota_bytes',(COALESCE(a.monthly_bytes,i.quota_bytes,9000000000000000000)+COALESCE(q.credit,0))::TEXT,
        'expires_at',NULLIF(COALESCE(a.expires_at,i.expires_at),253402300799),
        'inviter_id',m.inviter_id,'sponsor_required',m.sponsor_required,'first_purchase_at',m.first_purchase_at,'version',m.version
    ) ORDER BY u.id),'[]'::JSONB) INTO result
    FROM singbox_nexus_identities i JOIN users u ON u.id=i.user_id JOIN singbox_members m ON m.user_id=u.id JOIN singbox_wallets w ON w.user_id=u.id
    LEFT JOIN singbox_user_packages up ON up.user_id=u.id LEFT JOIN singbox_package_assignments a ON a.id=up.assignment_id
    LEFT JOIN singbox_nexus_plan_mappings p ON p.package_group_id=a.package_group_id AND p.import_id=bridge.import_id
    LEFT JOIN singbox_nexus_metering meter ON meter.user_id=u.id
    LEFT JOIN LATERAL (SELECT sum(bytes) AS credit FROM singbox_quota_credits WHERE user_id=u.id AND cycle_start=COALESCE(meter.last_reset_at,0) AND next_reset=COALESCE(meter.next_reset_at,253402300799)) q ON TRUE
    WHERE i.import_id=bridge.import_id AND m.state<>'staged' AND NOT EXISTS(SELECT 1 FROM singbox_native_adoptions adopted WHERE adopted.user_id=i.user_id);
    generation=COALESCE(bridge.pending_generation,gen_random_uuid());
    UPDATE singbox_native_adoptions a SET legacy_retired_generation=generation
        FROM singbox_nexus_identities i WHERE i.user_id=a.user_id AND i.import_id=bridge.import_id AND a.legacy_retired_at IS NULL;
    UPDATE singbox_nexus_bridge SET pending_generation=generation,native_control_ready=(bridge.pending_generation IS NOT NULL AND bridge.native_control_ready) WHERE singleton;
    RETURN jsonb_build_object('generation',generation,'members',result,'retired_members',(SELECT COALESCE(jsonb_agg(i.user_id ORDER BY i.user_id),'[]'::jsonb) FROM singbox_nexus_identities i JOIN singbox_native_adoptions a ON a.user_id=i.user_id WHERE i.import_id=bridge.import_id));
END $_$;

CREATE OR REPLACE FUNCTION public.singbox_nexus_device_exchange(sample jsonb) RETURNS jsonb
    LANGUAGE plpgsql
    AS $_$
DECLARE bridge singbox_nexus_bridge%ROWTYPE; observed BIGINT; now_s BIGINT; key TEXT; value JSONB; result JSONB;
BEGIN
    PERFORM pg_advisory_xact_lock(73201002);
    SELECT * INTO STRICT bridge FROM singbox_nexus_bridge WHERE singleton AND enabled;
    IF sample->>'import_id' IS DISTINCT FROM bridge.import_id::TEXT
        OR jsonb_typeof(sample->'limits') IS DISTINCT FROM 'object'
        OR jsonb_typeof(sample->'legacy') IS DISTINCT FROM 'object'
        OR COALESCE(sample->>'observed_at','') !~ '^[0-9]{1,12}$' THEN
        RAISE EXCEPTION 'Invalid device sample';
    END IF;
    now_s = EXTRACT(EPOCH FROM now())::BIGINT;
    observed = (sample->>'observed_at')::BIGINT;
    IF observed < now_s-120 OR observed > now_s+30 THEN RAISE EXCEPTION 'Expired device sample'; END IF;
    IF (SELECT count(*) FROM jsonb_object_keys(sample->'limits')) > 20000
        OR (SELECT count(*) FROM jsonb_object_keys(sample->'legacy')) > 20000 THEN
        RAISE EXCEPTION 'Invalid device sample';
    END IF;
    FOR key, value IN SELECT * FROM jsonb_each(sample->'limits') LOOP
        IF key !~ '^[1-9][0-9]{0,15}$' OR jsonb_typeof(value) <> 'number'
            OR value::TEXT !~ '^[0-9]{1,4}$' OR value::TEXT::INTEGER > 1024 THEN
            RAISE EXCEPTION 'Invalid device limit';
        END IF;
    END LOOP;
    FOR key, value IN SELECT * FROM jsonb_each(sample->'legacy') LOOP
        IF key !~ '^[1-9][0-9]{0,15}$' OR jsonb_typeof(value) <> 'array' OR jsonb_array_length(value) > 64
            OR EXISTS(SELECT 1 FROM jsonb_array_elements(value) a WHERE jsonb_typeof(a) <> 'string')
            OR EXISTS(SELECT 1 FROM jsonb_array_elements_text(value) a
                WHERE masklen(a::INET) <> CASE family(a::INET) WHEN 4 THEN 32 ELSE 128 END) THEN
            RAISE EXCEPTION 'Invalid legacy devices';
        END IF;
    END LOOP;
    INSERT INTO singbox_nexus_device_state(user_id,device_limit,legacy_addresses,observed_at)
        SELECT i.user_id, COALESCE((sample->'limits'->>i.user_id::TEXT)::INTEGER,0),
               COALESCE(sample->'legacy'->(i.user_id::TEXT),'[]'::JSONB), observed
        FROM singbox_nexus_identities i WHERE i.import_id=bridge.import_id AND NOT EXISTS(SELECT 1 FROM singbox_native_adoptions adopted WHERE adopted.user_id=i.user_id)
    ON CONFLICT(user_id) DO UPDATE SET device_limit=EXCLUDED.device_limit,
        legacy_addresses=EXCLUDED.legacy_addresses, observed_at=EXCLUDED.observed_at
        WHERE EXCLUDED.observed_at>=singbox_nexus_device_state.observed_at;
    -- Admitted addresses on taken-over nodes, keyed by their legacy node so the
    -- legacy panel counts them like its own nodes' devices. A node is healthy
    -- while its device is online and its applied runtime is certified, which
    -- keeps the legacy node status truthful after its old software stopped.
    SELECT COALESCE(jsonb_agg(jsonb_build_object('legacy_node_id',n.legacy_node_id,'users',n.users,'healthy',n.healthy)
        ORDER BY n.legacy_node_id),'[]'::JSONB) INTO result
    FROM (
        SELECT i.legacy_node_id, EXISTS(
            SELECT 1 FROM server_module_status m JOIN servers v ON v.id=m.server_id
            WHERE m.server_id=r.server_id AND m.module='singbox' AND m.healthy
              AND m.applied_rev=m.target_rev AND v.deleted_at IS NULL AND v.last_seen>=now_s-60) AS healthy,
            COALESCE((
            SELECT jsonb_object_agg(a.user_id::TEXT, a.addresses)
            FROM (SELECT user_id, jsonb_agg(DISTINCT address) AS addresses
                  FROM singbox_runtime_session_addresses
                  WHERE server_id=r.server_id AND node_id=r.node_id AND observed_at>=now_s-60 AND NOT EXISTS(SELECT 1 FROM singbox_native_adoptions adopted WHERE adopted.user_id=singbox_runtime_session_addresses.user_id)
                  GROUP BY user_id) a), '{}'::JSONB) AS users
        FROM singbox_nexus_runtime_nodes r
        JOIN singbox_nexus_runtime_hosts h ON h.server_id=r.server_id
        JOIN singbox_nexus_nodes i ON i.id=r.inventory_id
        WHERE h.state='active'
    ) n;
    RETURN jsonb_build_object('managed', result);
END;
$_$;

-- Refresh the applied subscription projection without changing a runtime bundle,
-- and reconcile any previously enabled server whose active service configuration is empty.
UPDATE runtime_publications p SET dirty_at=COALESCE(p.dirty_at,0)
    FROM singbox_nexus_runtime_hosts h WHERE h.server_id=p.server_id AND h.state='active';
INSERT INTO public.runtime_publications(server_id,manifest_rev,dirty_at)
    SELECT m.server_id,GREATEST(m.target_rev,0),0
    FROM public.server_module_status m
    JOIN public.servers s ON s.id=m.server_id
    WHERE m.module='singbox' AND m.target_rev>0 AND s.deleted_at IS NULL
ON CONFLICT(server_id) DO UPDATE SET dirty_at=COALESCE(public.runtime_publications.dirty_at,0);

DROP TABLE
    public.singbox_runtime_rollout_members,
    public.singbox_runtime_rollouts,
    public.singbox_runtime_manifest_facts,
    public.singbox_chain_runtime_requirements,
    public.singbox_installation,
    public.runtime_enablements;

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
    IF actual<>'3809eb993072dab6903cdcacf2bbfc3f' THEN RAISE EXCEPTION 'unexpected target structure: %',actual; END IF;
    UPDATE public.apsp_schema_baseline SET fingerprint=actual WHERE singleton;
    UPDATE public._sqlx_migrations SET checksum=decode('f067f482c1a209bc5074f1ee4e8d56e49f049bef74682bd342c4c2f38582e1a851448fdc0fe4f319a8f32ded655deba5','hex') WHERE version=1;
END $$;
