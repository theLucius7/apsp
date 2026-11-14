-- Current unreleased database baseline (ADR 0122). Rebuild older development databases manually.
SET LOCAL check_function_bodies = false;

CREATE FUNCTION public.apsp_traffic_cycle_start(at_time bigint, reset_day integer) RETURNS bigint
    LANGUAGE sql IMMUTABLE STRICT
    AS $$
    WITH current_month AS (
        SELECT date_trunc('month', to_timestamp(at_time) AT TIME ZONE 'UTC')::date AS month
    ), candidate AS (
        SELECT month, month + (LEAST(reset_day,
            EXTRACT(DAY FROM month + INTERVAL '1 month - 1 day')::integer) - 1) AS boundary
        FROM current_month
    ), selected_month AS (
        SELECT CASE WHEN (to_timestamp(at_time) AT TIME ZONE 'UTC')::date >= boundary
            THEN month ELSE (month - INTERVAL '1 month')::date END AS month
        FROM candidate
    )
    SELECT EXTRACT(EPOCH FROM ((month + (LEAST(reset_day,
        EXTRACT(DAY FROM month + INTERVAL '1 month - 1 day')::integer) - 1))::timestamp
        AT TIME ZONE 'UTC'))::bigint FROM selected_month;
$$;

CREATE FUNCTION public.singbox_check_external_metadata() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF NEW.kind <> 'external'
        OR EXISTS(SELECT 1 FROM singbox_ordered_external_nodes WHERE public_id=NEW.id) THEN
        RETURN NEW;
    END IF;
    RAISE EXCEPTION 'external node metadata refers to an unknown node' USING ERRCODE='23503';
END $$;

CREATE FUNCTION public.singbox_check_policy_external() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF EXISTS(
        SELECT 1 FROM singbox_ordered_external_nodes n
        JOIN singbox_ordered_external_node_versions v ON v.node_id=n.id
        WHERE n.public_id=NEW.external_node_id AND v.public_id=NEW.node_version_id
    ) THEN
        RETURN NEW;
    END IF;
    RAISE EXCEPTION 'policy group external node refers to an unknown node version' USING ERRCODE='23503';
END $$;

CREATE FUNCTION public.singbox_cycle_bounds(at_s bigint, reset_day integer, reset_hour integer, reset_minute integer, zone text) RETURNS TABLE(cycle_start bigint, next_reset bigint)
    LANGUAGE plpgsql STABLE STRICT
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

CREATE FUNCTION public.singbox_eligible_accesses(at_s bigint) RETURNS TABLE(user_id bigint, node_id bigint, uuid uuid, stat_name text, credential text)
    LANGUAGE sql STABLE
    AS $$
    SELECT a.user_id,a.node_id,a.uuid,a.stat_name,a.credential FROM accesses a
    JOIN singbox_desired_accesses d USING(user_id,node_id)
    JOIN singbox_entitlements(at_s) e ON e.user_id=a.user_id AND e.allowed
    JOIN nodes n ON n.id=a.node_id AND n.enabled
    WHERE NOT EXISTS(SELECT 1 FROM singbox_chains c WHERE c.entry_node_id=n.id AND c.deleted_at IS NULL
        AND EXISTS(SELECT 1 FROM singbox_ordered_chain_hops h JOIN nodes x ON x.id=h.managed_node_id
            WHERE h.chain_id=c.id AND h.generation=ANY(ARRAY[c.desired_generation,c.applied_generation,c.candidate_generation,c.recovery_generation]) AND NOT x.enabled));
$$;

CREATE FUNCTION public.singbox_entitlements(at_s bigint) RETURNS TABLE(user_id bigint, package_group_id bigint, package_name text, monthly_bytes text, reset_day integer, reset_hour integer, reset_minute integer, timezone text, starts_at bigint, expires_at bigint, cycle_start bigint, next_reset bigint, used_bytes text, status text, allowed boolean)
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

CREATE FUNCTION public.singbox_legacy_policy_group(legacy bigint) RETURNS bigint
    LANGUAGE plpgsql
    AS $$
DECLARE
    found BIGINT;
    label TEXT;
BEGIN
    SELECT id INTO found FROM singbox_policy_groups WHERE legacy_group_id=legacy;
    IF found IS NOT NULL THEN
        RETURN found;
    END IF;
    SELECT NULLIF(btrim(g->>'name'),'') INTO label
    FROM singbox_nexus_nodes n
    CROSS JOIN LATERAL jsonb_array_elements(
        CASE WHEN jsonb_typeof(n.display->'groups')='array' THEN n.display->'groups' ELSE '[]'::JSONB END) g
    WHERE g->>'id'=legacy::TEXT AND NULLIF(btrim(g->>'name'),'') IS NOT NULL
    ORDER BY n.id LIMIT 1;
    INSERT INTO singbox_policy_groups(name,legacy_group_id)
        VALUES (left(COALESCE('原权限组 · '||label,'原权限组 #'||legacy),128),legacy)
        ON CONFLICT (legacy_group_id) DO NOTHING RETURNING id INTO found;
    IF found IS NULL THEN
        SELECT id INTO found FROM singbox_policy_groups WHERE legacy_group_id=legacy;
    END IF;
    RETURN found;
END $$;

CREATE FUNCTION public.singbox_member_legacy_group(member bigint) RETURNS bigint
    LANGUAGE sql STABLE
    AS $$
    SELECT g.legacy_group_id FROM singbox_user_policies up
    JOIN singbox_policy_groups g ON g.id=up.group_id AND g.legacy_group_id IS NOT NULL
    LEFT JOIN singbox_user_packages pk ON pk.user_id=up.user_id
    LEFT JOIN singbox_package_assignments a ON a.id=pk.assignment_id
    LEFT JOIN singbox_nexus_plan_mappings p ON p.package_group_id=a.package_group_id
    WHERE up.user_id=member
    ORDER BY g.legacy_group_id IS NOT DISTINCT FROM p.legacy_group_id DESC, g.legacy_group_id
    LIMIT 1
$$;

CREATE FUNCTION public.singbox_nexus_bridge_exchange(sample jsonb) RETURNS jsonb
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
        IF EXISTS(SELECT 1 FROM singbox_nexus_identities WHERE user_id=(entry->>'id')::BIGINT AND import_id=bridge.import_id) THEN
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
        WHERE m.state<>'staged' AND u.deleted_at IS NULL ON CONFLICT(user_id) DO NOTHING;
    IF EXISTS(SELECT 1 FROM singbox_members m JOIN singbox_user_packages up ON up.user_id=m.user_id JOIN singbox_package_assignments a ON a.id=up.assignment_id
        JOIN singbox_nexus_identities i ON i.user_id=m.user_id LEFT JOIN singbox_nexus_plan_mappings p ON p.package_group_id=a.package_group_id
        WHERE i.import_id=bridge.import_id AND m.state='active' AND (p.package_group_id IS NULL OR p.legacy_group_id IS NULL)) THEN
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
    WHERE i.import_id=bridge.import_id AND m.state<>'staged';
    generation=gen_random_uuid();
    UPDATE singbox_nexus_bridge SET pending_generation=generation WHERE singleton;
    RETURN jsonb_build_object('generation',generation,'members',result);
END $_$;

CREATE FUNCTION public.singbox_nexus_device_exchange(sample jsonb) RETURNS jsonb
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
        FROM singbox_nexus_identities i WHERE i.import_id=bridge.import_id
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
                  WHERE server_id=r.server_id AND node_id=r.node_id AND observed_at>=now_s-60
                  GROUP BY user_id) a), '{}'::JSONB) AS users
        FROM singbox_nexus_runtime_nodes r
        JOIN singbox_nexus_runtime_hosts h ON h.server_id=r.server_id
        JOIN singbox_nexus_nodes i ON i.id=r.inventory_id
        WHERE h.state='active'
    ) n;
    RETURN jsonb_build_object('managed', result);
END;
$_$;

CREATE FUNCTION public.singbox_nexus_guard_access() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF EXISTS(SELECT 1 FROM singbox_nexus_runtime_nodes WHERE node_id IN (NEW.node_id,OLD.node_id)) THEN
        RAISE EXCEPTION 'Nexus runtime lines keep the original client identities; grant them through policy groups';
    END IF;
    IF TG_OP='DELETE' THEN RETURN OLD; ELSE RETURN NEW; END IF;
END $$;

CREATE FUNCTION public.singbox_nexus_guard_node() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF current_setting('apsp.nexus_runtime_write',TRUE) IS DISTINCT FROM 'yes' AND
       EXISTS(SELECT 1 FROM singbox_nexus_runtime_hosts WHERE server_id IN (NEW.server_id,OLD.server_id)) THEN
        RAISE EXCEPTION 'Nexus runtime nodes are managed by their preserved configuration';
    END IF;
    IF TG_OP='DELETE' THEN RETURN OLD; ELSE RETURN NEW; END IF;
END $$;

CREATE FUNCTION public.singbox_nexus_node_status_exchange(sample jsonb) RETURNS jsonb
    LANGUAGE plpgsql
    AS $_$
DECLARE bridge singbox_nexus_bridge%ROWTYPE; observed BIGINT; now_s BIGINT; entry JSONB; carrier TEXT; measured JSONB; key TEXT; ids BIGINT[] := '{}';
BEGIN
    PERFORM pg_advisory_xact_lock(73201003);
    SELECT * INTO STRICT bridge FROM singbox_nexus_bridge WHERE singleton AND enabled;
    IF sample->>'import_id' IS DISTINCT FROM bridge.import_id::TEXT
        OR jsonb_typeof(sample->'nodes') IS DISTINCT FROM 'array'
        OR jsonb_array_length(sample->'nodes') > 5000
        OR COALESCE(sample->>'observed_at','') !~ '^[0-9]{1,12}$' THEN
        RAISE EXCEPTION 'Invalid node status sample';
    END IF;
    now_s = EXTRACT(EPOCH FROM now())::BIGINT;
    observed = (sample->>'observed_at')::BIGINT;
    IF observed < now_s-120 OR observed > now_s+30 THEN RAISE EXCEPTION 'Expired node status sample'; END IF;
    FOR entry IN SELECT value FROM jsonb_array_elements(sample->'nodes') LOOP
        IF COALESCE(entry->>'id','') !~ '^[1-9][0-9]{0,15}$' OR jsonb_typeof(entry->'online') <> 'boolean'
            OR (entry ? 'checked_at' AND jsonb_typeof(entry->'checked_at') NOT IN ('number','null'))
            OR jsonb_typeof(COALESCE(entry->'load','{}')) <> 'object'
            OR jsonb_typeof(COALESCE(entry->'latency','{}')) <> 'object'
            OR jsonb_typeof(COALESCE(entry->'latency'->'carriers','{}')) <> 'object' THEN
            RAISE EXCEPTION 'Invalid node status';
        END IF;
        -- Only numeric load percentages and the three carrier measurements.
        FOR key IN SELECT jsonb_object_keys(COALESCE(entry->'load','{}')) LOOP
            IF key NOT IN ('cpu','memory','disk') OR jsonb_typeof(entry->'load'->key) NOT IN ('number','null') THEN
                RAISE EXCEPTION 'Invalid node load';
            END IF;
        END LOOP;
        FOR carrier, measured IN SELECT * FROM jsonb_each(COALESCE(entry->'latency'->'carriers','{}')) LOOP
            IF carrier NOT IN ('telecom','unicom','mobile') OR jsonb_typeof(measured) <> 'object'
                OR EXISTS(SELECT 1 FROM jsonb_each(measured) m WHERE m.key NOT IN ('latest','average','loss') OR jsonb_typeof(m.value) NOT IN ('number','null')) THEN
                RAISE EXCEPTION 'Invalid node latency';
            END IF;
        END LOOP;
        ids = ids || (entry->>'id')::BIGINT;
        INSERT INTO singbox_nexus_node_status(legacy_node_id,online,checked_at,load,latency,observed_at)
        VALUES((entry->>'id')::BIGINT, (entry->>'online')::BOOLEAN, NULLIF(entry->>'checked_at','')::BIGINT,
               COALESCE(entry->'load','{}'),
               jsonb_build_object('monitored', COALESCE((entry->'latency'->>'monitored')::BOOLEAN,false),
                   'updated_at', CASE WHEN jsonb_typeof(entry->'latency'->'updated_at')='number' THEN entry->'latency'->'updated_at' ELSE 'null'::JSONB END,
                   'carriers', COALESCE(entry->'latency'->'carriers','{}')),
               observed)
        ON CONFLICT(legacy_node_id) DO UPDATE SET online=EXCLUDED.online, checked_at=EXCLUDED.checked_at,
            load=EXCLUDED.load, latency=EXCLUDED.latency, observed_at=EXCLUDED.observed_at
            WHERE EXCLUDED.observed_at>=singbox_nexus_node_status.observed_at;
    END LOOP;
    -- Nodes the legacy panel no longer lists have no current status.
    DELETE FROM singbox_nexus_node_status WHERE legacy_node_id <> ALL(ids);
    RETURN jsonb_build_object('nodes', cardinality(ids));
END;
$_$;

CREATE FUNCTION public.singbox_reject_path_snapshot_change() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    RAISE EXCEPTION 'immutable path snapshot' USING ERRCODE='55000';
END;
$$;

CREATE FUNCTION public.singbox_reject_subscription_snapshot_change() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    RAISE EXCEPTION 'immutable subscription snapshot' USING ERRCODE='55000';
END;
$$;

CREATE FUNCTION public.singbox_usage_daily_apply() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    INSERT INTO singbox_usage_daily(user_id, day, node_id, uplink, downlink, records, last_period_end)
    SELECT user_id, period_end / 86400 * 86400, node_id, SUM(uplink), SUM(downlink), COUNT(*), MAX(period_end)
    FROM inserted
    GROUP BY user_id, period_end / 86400 * 86400, node_id
    ON CONFLICT (user_id, day, node_id) DO UPDATE SET
        uplink = singbox_usage_daily.uplink + EXCLUDED.uplink,
        downlink = singbox_usage_daily.downlink + EXCLUDED.downlink,
        records = singbox_usage_daily.records + EXCLUDED.records,
        last_period_end = GREATEST(singbox_usage_daily.last_period_end, EXCLUDED.last_period_end);
    RETURN NULL;
END;
$$;

CREATE FUNCTION public.singbox_usage_daily_rebuild() RETURNS bigint
    LANGUAGE plpgsql
    AS $$
DECLARE
    rebuilt BIGINT;
BEGIN
    LOCK TABLE usage_records IN SHARE ROW EXCLUSIVE MODE;
    DELETE FROM singbox_usage_daily;
    INSERT INTO singbox_usage_daily(user_id, day, node_id, uplink, downlink, records, last_period_end)
    SELECT user_id, period_end / 86400 * 86400, node_id, SUM(uplink), SUM(downlink), COUNT(*), MAX(period_end)
    FROM usage_records
    GROUP BY user_id, period_end / 86400 * 86400, node_id;
    GET DIAGNOSTICS rebuilt = ROW_COUNT;
    RETURN rebuilt;
END;
$$;

CREATE FUNCTION public.singbox_usage_window(p_user bigint, p_after bigint, p_until bigint) RETURNS numeric
    LANGUAGE sql STABLE STRICT
    AS $$
    WITH bounds AS (
        SELECT (ceil((p_after + 1)::NUMERIC / 86400) * 86400)::BIGINT AS lo,
            (floor((p_until + 1)::NUMERIC / 86400) * 86400)::BIGINT AS hi
    )
    SELECT CASE
        WHEN p_until <= p_after THEN 0::NUMERIC
        -- Day buckets truncate toward zero, so negative instants use the ledger only.
        WHEN p_after < 0 THEN COALESCE((SELECT SUM(r.uplink + r.downlink) FROM usage_records r
            WHERE r.user_id = p_user AND r.period_end > p_after AND r.period_end <= p_until), 0)
        ELSE COALESCE((SELECT SUM(d.uplink + d.downlink) FROM singbox_usage_daily d
                WHERE d.user_id = p_user AND d.day >= b.lo AND d.day < b.hi), 0)
            + COALESCE((SELECT SUM(r.uplink + r.downlink) FROM usage_records r
                WHERE r.user_id = p_user AND r.period_end > p_after
                    AND r.period_end < LEAST(b.lo, p_until + 1)), 0)
            + COALESCE((SELECT SUM(r.uplink + r.downlink) FROM usage_records r
                WHERE r.user_id = p_user AND r.period_end >= GREATEST(b.lo, b.hi)
                    AND r.period_end <= p_until), 0)
        END
    FROM bounds b
$$;

CREATE TABLE public.accesses (
    user_id bigint NOT NULL,
    node_id bigint NOT NULL,
    uuid uuid NOT NULL,
    stat_name text NOT NULL,
    credential text DEFAULT ''::text NOT NULL
);

CREATE SEQUENCE public.admin_identity_seq
    START WITH 2
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

CREATE TABLE public.administrator_passkey_accounts (
    admin_id bigint NOT NULL,
    account_id uuid NOT NULL
);

CREATE TABLE public.administrator_preferences (
    admin_id bigint NOT NULL,
    preference_key text NOT NULL,
    value jsonb NOT NULL,
    revision bigint DEFAULT 1 NOT NULL,
    updated_at bigint NOT NULL
);

CREATE TABLE public.administrator_profiles (
    admin_id bigint NOT NULL,
    login_name text NOT NULL,
    display_name text NOT NULL,
    role text NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    all_servers boolean DEFAULT false NOT NULL,
    capabilities jsonb DEFAULT '[]'::jsonb NOT NULL,
    revision bigint DEFAULT 1 NOT NULL,
    created_at bigint NOT NULL,
    updated_at bigint NOT NULL,
    CONSTRAINT administrator_profiles_display_name_check CHECK (((octet_length(display_name) >= 1) AND (octet_length(display_name) <= 200))),
    CONSTRAINT administrator_profiles_login_name_check CHECK (((octet_length(login_name) >= 1) AND (octet_length(login_name) <= 100))),
    CONSTRAINT administrator_profiles_revision_check CHECK ((revision > 0)),
    CONSTRAINT administrator_profiles_role_check CHECK ((role = ANY (ARRAY['owner'::text, 'operator'::text, 'viewer'::text])))
);

CREATE TABLE public.administrator_reauth (
    session_hash text NOT NULL,
    verified_at bigint NOT NULL,
    expires_at bigint NOT NULL
);

CREATE TABLE public.administrator_server_grants (
    admin_id bigint NOT NULL,
    server_id bigint NOT NULL
);

CREATE TABLE public.admins (
    id bigint DEFAULT nextval('public.admin_identity_seq'::regclass) NOT NULL,
    password_hash text NOT NULL,
    totp_secret bytea,
    totp_last_step bigint,
    totp_pending_secret bytea,
    totp_pending_expires bigint,
    totp_pending_session text,
    CONSTRAINT admins_totp_last_step_check CHECK (((totp_last_step IS NULL) OR (totp_last_step >= 0))),
    CONSTRAINT admins_totp_pending_secret_check CHECK (((totp_pending_secret IS NULL) OR (octet_length(totp_pending_secret) = 20))),
    CONSTRAINT admins_totp_secret_check CHECK (((totp_secret IS NULL) OR (octet_length(totp_secret) = 20)))
);

CREATE TABLE public.alert_reminder_receipts (
    server_id bigint NOT NULL,
    source_key text NOT NULL,
    retain_until bigint NOT NULL
);

CREATE TABLE public.alert_rules (
    id uuid NOT NULL,
    spec jsonb NOT NULL,
    revision bigint DEFAULT 1 NOT NULL
);

CREATE TABLE public.alicloud_accounts (
    id uuid NOT NULL,
    name text NOT NULL,
    site text DEFAULT 'china'::text NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    auto_enabled boolean DEFAULT false NOT NULL,
    limit_gb bigint DEFAULT 100 NOT NULL,
    revision bigint DEFAULT 1 NOT NULL,
    next_run_at bigint DEFAULT 0 NOT NULL,
    last_attempt_at bigint DEFAULT 0 NOT NULL,
    archived boolean DEFAULT false NOT NULL,
    credential_id uuid,
    CONSTRAINT alicloud_accounts_limit_gb_check CHECK (((limit_gb >= 1) AND (limit_gb <= 1000000000))),
    CONSTRAINT alicloud_accounts_site_check CHECK ((site = ANY (ARRAY['china'::text, 'international'::text])))
);

CREATE TABLE public.alicloud_deliveries (
    id bigint NOT NULL,
    event_id bigint NOT NULL,
    channel text NOT NULL,
    payload text NOT NULL,
    status text DEFAULT 'pending'::text NOT NULL,
    attempts integer DEFAULT 0 NOT NULL,
    next_attempt_at bigint NOT NULL,
    last_error text,
    delivered_at bigint,
    CONSTRAINT alicloud_deliveries_channel_check CHECK ((channel = ANY (ARRAY['telegram'::text, 'webhook'::text]))),
    CONSTRAINT alicloud_deliveries_status_check CHECK ((status = ANY (ARRAY['pending'::text, 'sent'::text, 'failed'::text, 'cancelled'::text])))
);

CREATE SEQUENCE public.alicloud_deliveries_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

ALTER SEQUENCE public.alicloud_deliveries_id_seq OWNED BY public.alicloud_deliveries.id;

CREATE TABLE public.alicloud_eip_rotations (
    id uuid NOT NULL,
    resource_id uuid NOT NULL,
    requested_by bigint NOT NULL,
    before_state jsonb NOT NULL,
    probe_port integer NOT NULL,
    max_attempts integer NOT NULL,
    snapshot_digest text NOT NULL,
    status text NOT NULL,
    phase text DEFAULT 'allocate'::text NOT NULL,
    phase_started_at bigint DEFAULT 0 NOT NULL,
    attempt integer DEFAULT 1 NOT NULL,
    bound_id text NOT NULL,
    candidate jsonb,
    measurement_id text,
    drop_ids text[] DEFAULT '{}'::text[] NOT NULL,
    after_drop text,
    attempts jsonb DEFAULT '[]'::jsonb NOT NULL,
    new_ip text,
    error_code text,
    created_at bigint NOT NULL,
    expires_at bigint NOT NULL,
    updated_at bigint NOT NULL,
    next_check_at bigint DEFAULT 0 NOT NULL,
    reconciliation jsonb,
    version bigint DEFAULT 0 NOT NULL,
    CONSTRAINT alicloud_eip_rotations_max_attempts_check CHECK (((max_attempts >= 1) AND (max_attempts <= 12))),
    CONSTRAINT alicloud_eip_rotations_probe_port_check CHECK (((probe_port >= 1) AND (probe_port <= 65535))),
    CONSTRAINT alicloud_eip_rotations_status_check CHECK ((status = ANY (ARRAY['preview'::text, 'cancelled'::text, 'running'::text, 'unknown'::text, 'succeeded'::text, 'restored'::text, 'failed'::text, 'reconciled'::text])))
);

CREATE TABLE public.alicloud_events (
    id bigint NOT NULL,
    resource_id uuid NOT NULL,
    dedup_key text NOT NULL,
    title text NOT NULL,
    message text NOT NULL,
    created_at bigint NOT NULL
);

CREATE SEQUENCE public.alicloud_events_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

ALTER SEQUENCE public.alicloud_events_id_seq OWNED BY public.alicloud_events.id;

CREATE TABLE public.alicloud_managed_security_groups (
    resource_id uuid NOT NULL,
    group_id text NOT NULL,
    group_digest text NOT NULL,
    operation_id uuid NOT NULL,
    confirmed_at bigint NOT NULL
);

CREATE TABLE public.alicloud_operations (
    id uuid NOT NULL,
    resource_id uuid NOT NULL,
    account_revision bigint NOT NULL,
    resource_revision bigint NOT NULL,
    before_state jsonb NOT NULL,
    target jsonb NOT NULL,
    source text NOT NULL,
    billing_cycle text,
    status text NOT NULL,
    created_at bigint NOT NULL,
    expires_at bigint NOT NULL,
    updated_at bigint NOT NULL,
    next_check_at bigint DEFAULT 0 NOT NULL,
    error_code text,
    request_id text,
    CONSTRAINT alicloud_operations_source_check CHECK ((source = ANY (ARRAY['manual'::text, 'automatic'::text]))),
    CONSTRAINT alicloud_operations_status_check CHECK ((status = ANY (ARRAY['preview'::text, 'queued'::text, 'running'::text, 'uncertain'::text, 'succeeded'::text, 'failed'::text, 'cancelled'::text, 'dismissed'::text])))
);

CREATE TABLE public.alicloud_power_jobs (
    id uuid NOT NULL,
    resource_id uuid NOT NULL,
    account_revision bigint NOT NULL,
    resource_revision bigint NOT NULL,
    action text NOT NULL,
    stop_mode text NOT NULL,
    source text NOT NULL,
    dedup_key text,
    before_state jsonb NOT NULL,
    status text NOT NULL,
    created_at bigint NOT NULL,
    expires_at bigint NOT NULL,
    updated_at bigint NOT NULL,
    next_check_at bigint DEFAULT 0 NOT NULL,
    request_id text,
    error_code text,
    CONSTRAINT alicloud_power_jobs_action_check CHECK ((action = ANY (ARRAY['start'::text, 'stop'::text]))),
    CONSTRAINT alicloud_power_jobs_source_check CHECK ((source = ANY (ARRAY['manual'::text, 'threshold'::text, 'schedule'::text, 'keepalive'::text, 'cdt_guard'::text]))),
    CONSTRAINT alicloud_power_jobs_status_check CHECK ((status = ANY (ARRAY['preview'::text, 'queued'::text, 'running'::text, 'uncertain'::text, 'succeeded'::text, 'failed'::text, 'cancelled'::text, 'dismissed'::text]))),
    CONSTRAINT alicloud_power_jobs_stop_mode_check CHECK ((stop_mode = ANY (ARRAY['KeepCharging'::text, 'StopCharging'::text])))
);

CREATE TABLE public.alicloud_resources (
    id uuid NOT NULL,
    account_id uuid NOT NULL,
    name text NOT NULL,
    kind text NOT NULL,
    region text NOT NULL,
    cloud_id text NOT NULL,
    auto_enabled boolean DEFAULT false NOT NULL,
    cap_mbps bigint DEFAULT 1 NOT NULL,
    revision bigint DEFAULT 1 NOT NULL,
    snapshot jsonb,
    checked_at bigint,
    last_attempt_at bigint DEFAULT 0 NOT NULL,
    error_code text,
    archived boolean DEFAULT false NOT NULL,
    power_policy jsonb DEFAULT '{"enabled": false, "limit_gb": 100, "keepalive": false, "stop_mode": "KeepCharging", "stop_time": "23:00", "start_time": "08:00", "schedule_enabled": false, "threshold_action": "off", "threshold_percent": 95, "utc_offset_minutes": 480}'::jsonb NOT NULL,
    power_state jsonb,
    power_checked_at bigint,
    power_error text,
    next_power_at bigint DEFAULT 0 NOT NULL,
    manual_hold boolean DEFAULT false NOT NULL,
    threshold_hold boolean DEFAULT false NOT NULL,
    cdt_guard_state jsonb,
    cdt_guard_error text,
    cdt_guard_next_at bigint DEFAULT 0 NOT NULL,
    ipv6_addresses jsonb DEFAULT '[]'::jsonb NOT NULL,
    ipv6_checked_at bigint,
    ipv6_error text,
    CONSTRAINT alicloud_resources_cap_mbps_check CHECK (((cap_mbps >= 1) AND (cap_mbps <= 100))),
    CONSTRAINT alicloud_resources_kind_check CHECK ((kind = ANY (ARRAY['ecs'::text, 'eip'::text])))
);

CREATE TABLE public.alicloud_security_group_operations (
    id uuid NOT NULL,
    resource_id uuid NOT NULL,
    requested_by bigint NOT NULL,
    before_state jsonb NOT NULL,
    target_groups text[] NOT NULL,
    impact jsonb NOT NULL,
    snapshot_digest text NOT NULL,
    status text NOT NULL,
    steps jsonb DEFAULT '[]'::jsonb NOT NULL,
    created_at bigint NOT NULL,
    expires_at bigint NOT NULL,
    updated_at bigint NOT NULL,
    observed_at bigint,
    actual_groups text[],
    error_code text,
    original_result text,
    reconciliation jsonb,
    CONSTRAINT alicloud_security_group_operations_status_check CHECK ((status = ANY (ARRAY['preview'::text, 'running'::text, 'unknown'::text, 'succeeded'::text, 'failed'::text, 'reconciled'::text]))),
    CONSTRAINT alicloud_security_group_operations_target_groups_check CHECK (((cardinality(target_groups) >= 1) AND (cardinality(target_groups) <= 16)))
);

CREATE TABLE public.alicloud_traffic_history (
    account_id uuid NOT NULL,
    bucket_seconds integer NOT NULL,
    bucket_start bigint NOT NULL,
    sampled_at bigint NOT NULL,
    mainland_bytes numeric(20,0) NOT NULL,
    overseas_bytes numeric(20,0) NOT NULL,
    CONSTRAINT alicloud_traffic_history_bucket_seconds_check CHECK ((bucket_seconds = ANY (ARRAY[3600, 86400]))),
    CONSTRAINT alicloud_traffic_history_mainland_bytes_check CHECK ((mainland_bytes >= (0)::numeric)),
    CONSTRAINT alicloud_traffic_history_overseas_bytes_check CHECK ((overseas_bytes >= (0)::numeric))
);

CREATE TABLE public.auth_rate_limits (
    scope text NOT NULL,
    window_start bigint NOT NULL,
    attempts integer NOT NULL,
    CONSTRAINT auth_rate_limits_attempts_check CHECK ((attempts >= 0))
);

CREATE TABLE public.credential_entries (
    id uuid NOT NULL,
    name text NOT NULL,
    kind text NOT NULL,
    key_id text NOT NULL,
    nonce bytea NOT NULL,
    ciphertext bytea NOT NULL,
    version bigint DEFAULT 1 NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    created_at bigint NOT NULL,
    updated_at bigint NOT NULL,
    rotated_at bigint,
    CONSTRAINT credential_entries_kind_check CHECK ((kind = ANY (ARRAY['dns'::text, 'cloud'::text, 'backup'::text, 'external-api'::text, 'certificate'::text]))),
    CONSTRAINT credential_entries_nonce_check CHECK ((octet_length(nonce) = 12)),
    CONSTRAINT credential_entries_version_check CHECK ((version > 0))
);

CREATE TABLE public.ddns_history (
    id uuid NOT NULL,
    rule_id uuid NOT NULL,
    server_id bigint NOT NULL,
    revision bigint NOT NULL,
    operation text NOT NULL,
    desired_ip text,
    previous jsonb,
    observed jsonb,
    status text NOT NULL,
    error_code text,
    occurred_at bigint NOT NULL
);

CREATE TABLE public.ddns_migration_previews (
    id uuid NOT NULL,
    target_server_id bigint NOT NULL,
    snapshot jsonb NOT NULL,
    created_at bigint NOT NULL,
    expires_at bigint NOT NULL,
    applied_at bigint
);

CREATE TABLE public.ddns_rules (
    id uuid NOT NULL,
    server_id bigint NOT NULL,
    config jsonb NOT NULL,
    api_token text NOT NULL,
    revision bigint DEFAULT 1 NOT NULL,
    record_id text,
    last_ip text,
    last_success_at bigint,
    attempted_at bigint,
    next_run_at bigint DEFAULT 0 NOT NULL,
    failures integer DEFAULT 0 NOT NULL,
    status text DEFAULT 'pending'::text NOT NULL,
    error_code text,
    lease_id uuid,
    lease_until bigint DEFAULT 0 NOT NULL,
    access_key_id text DEFAULT ''::text NOT NULL,
    access_key_secret text DEFAULT ''::text NOT NULL,
    CONSTRAINT ddns_rules_check CHECK ((((config ->> 'server_id'::text))::bigint = server_id))
);

CREATE TABLE public.deployments (
    server_id bigint NOT NULL,
    module text NOT NULL,
    rev bigint NOT NULL,
    bundle text NOT NULL,
    bundle_sha256 text NOT NULL,
    source_json jsonb DEFAULT '[]'::jsonb NOT NULL,
    created_at bigint NOT NULL,
    CONSTRAINT deployments_rev_check CHECK ((rev > 0))
);

CREATE TABLE public.diagnostic_jobs (
    id uuid NOT NULL,
    server_id bigint NOT NULL,
    job jsonb NOT NULL,
    status text DEFAULT 'queued'::text NOT NULL,
    report jsonb,
    error text,
    agent_completed boolean DEFAULT false NOT NULL,
    created_at bigint NOT NULL,
    updated_at bigint NOT NULL,
    expires_at bigint NOT NULL,
    cancel_requested_at bigint,
    cancel_confirmed_at bigint,
    cancel_error text,
    expected_sections text[] DEFAULT '{}'::text[] NOT NULL,
    report_completeness text DEFAULT 'empty'::text NOT NULL,
    generation bigint NOT NULL,
    CONSTRAINT diagnostic_jobs_report_completeness_check CHECK ((report_completeness = ANY (ARRAY['empty'::text, 'partial'::text, 'complete'::text, 'legacy'::text]))),
    CONSTRAINT diagnostic_jobs_status_check CHECK ((status = ANY (ARRAY['queued'::text, 'running'::text, 'cleaning'::text, 'cancel_requested'::text, 'cancelled'::text, 'succeeded'::text, 'failed'::text])))
);

CREATE SEQUENCE public.diagnostic_jobs_generation_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

ALTER SEQUENCE public.diagnostic_jobs_generation_seq OWNED BY public.diagnostic_jobs.generation;

CREATE TABLE public.diagnostic_report_sections (
    job_id uuid NOT NULL,
    name text NOT NULL,
    text text NOT NULL,
    complete boolean NOT NULL,
    revision bigint NOT NULL,
    collected_at bigint NOT NULL,
    received_at bigint NOT NULL,
    CONSTRAINT diagnostic_report_sections_collected_at_check CHECK ((collected_at > 0)),
    CONSTRAINT diagnostic_report_sections_name_check CHECK ((name ~ '^[a-z0-9_]{1,64}$'::text)),
    CONSTRAINT diagnostic_report_sections_revision_check CHECK ((revision > 0)),
    CONSTRAINT diagnostic_report_sections_text_check CHECK (((octet_length(text) >= 1) AND (octet_length(text) <= 65536)))
);

CREATE TABLE public.dns_accounts (
    id uuid NOT NULL,
    config jsonb NOT NULL,
    revision bigint DEFAULT 1 NOT NULL,
    checked_at bigint,
    error_code text,
    created_at bigint NOT NULL,
    updated_at bigint NOT NULL
);

CREATE TABLE public.dns_record_history (
    id uuid NOT NULL,
    account_id uuid NOT NULL,
    account_revision bigint NOT NULL,
    request jsonb NOT NULL,
    previous jsonb,
    observed jsonb,
    status text NOT NULL,
    error_code text,
    occurred_at bigint NOT NULL,
    requested_by bigint,
    account_snapshot jsonb,
    credential_version bigint,
    preview_id uuid,
    rollback_of uuid,
    write_started_at bigint,
    rollback_started_at bigint
);

CREATE TABLE public.dns_record_previews (
    id uuid NOT NULL,
    account_id uuid NOT NULL,
    account_revision bigint NOT NULL,
    request jsonb NOT NULL,
    snapshot jsonb NOT NULL,
    created_at bigint NOT NULL,
    expires_at bigint NOT NULL,
    applied_at bigint
);

CREATE TABLE public.dns_resolver_observations (
    id uuid NOT NULL,
    account_id uuid NOT NULL,
    request jsonb NOT NULL,
    result jsonb NOT NULL,
    occurred_at bigint NOT NULL
);

CREATE TABLE public.enrollment_tokens (
    token_hash text NOT NULL,
    server_id bigint NOT NULL,
    expires_at bigint NOT NULL,
    consumed_at bigint
);

CREATE TABLE public.exchange_rates (
    singleton boolean DEFAULT true NOT NULL,
    base text DEFAULT 'CNY'::text NOT NULL,
    rates jsonb DEFAULT '{"CNY": 1}'::jsonb NOT NULL,
    rate_dates jsonb DEFAULT '{}'::jsonb NOT NULL,
    source text,
    rate_date text,
    fetched_at bigint,
    attempted_at bigint,
    next_refresh_at bigint DEFAULT 0 NOT NULL,
    lease_id uuid,
    lease_until bigint DEFAULT 0 NOT NULL,
    last_error text,
    CONSTRAINT exchange_rates_base_check CHECK ((base = 'CNY'::text)),
    CONSTRAINT exchange_rates_singleton_check CHECK (singleton),
    CONSTRAINT exchange_rates_source_check CHECK ((source = ANY (ARRAY['frankfurter'::text, 'frankfurter-ecb'::text])))
);

CREATE TABLE public.expense_deliveries (
    id bigint NOT NULL,
    event_id uuid NOT NULL,
    channel text NOT NULL,
    payload text NOT NULL,
    state text DEFAULT 'pending'::text NOT NULL,
    attempts integer DEFAULT 0 NOT NULL,
    next_attempt_at bigint NOT NULL,
    lease_until bigint,
    last_error text,
    CONSTRAINT expense_deliveries_channel_check CHECK ((channel = ANY (ARRAY['telegram'::text, 'webhook'::text]))),
    CONSTRAINT expense_deliveries_state_check CHECK ((state = ANY (ARRAY['pending'::text, 'sending'::text, 'sent'::text, 'failed'::text, 'cancelled'::text])))
);

CREATE SEQUENCE public.expense_deliveries_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

ALTER SEQUENCE public.expense_deliveries_id_seq OWNED BY public.expense_deliveries.id;

CREATE TABLE public.expense_events (
    id uuid NOT NULL,
    subscription_id bigint,
    event_key text NOT NULL,
    kind text NOT NULL,
    detail jsonb NOT NULL,
    created_at bigint NOT NULL
);

CREATE TABLE public.expense_imports (
    id uuid NOT NULL,
    source_id text NOT NULL,
    digest text NOT NULL,
    receipt jsonb NOT NULL,
    created_at bigint NOT NULL
);

CREATE TABLE public.expense_labels (
    id bigint NOT NULL,
    kind text NOT NULL,
    name text NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    CONSTRAINT expense_labels_kind_check CHECK ((kind = ANY (ARRAY['category'::text, 'payment_method'::text, 'payer'::text])))
);

CREATE SEQUENCE public.expense_labels_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

ALTER SEQUENCE public.expense_labels_id_seq OWNED BY public.expense_labels.id;

CREATE TABLE public.expense_payments (
    id bigint NOT NULL,
    subscription_id bigint NOT NULL,
    request_id uuid NOT NULL,
    period_start date NOT NULL,
    paid_on date NOT NULL,
    amount_units bigint NOT NULL,
    currency text NOT NULL,
    administrator_id bigint NOT NULL,
    created_at bigint NOT NULL,
    CONSTRAINT expense_payments_amount_units_check CHECK (((amount_units >= 0) AND (amount_units <= '9000000000000000'::bigint))),
    CONSTRAINT expense_payments_currency_check CHECK ((currency ~ '^[A-Z]{3}$'::text))
);

CREATE SEQUENCE public.expense_payments_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

ALTER SEQUENCE public.expense_payments_id_seq OWNED BY public.expense_payments.id;

CREATE TABLE public.expense_settings (
    singleton boolean DEFAULT true NOT NULL,
    base_currency text DEFAULT 'USD'::text NOT NULL,
    budget_units bigint,
    timezone text DEFAULT 'UTC'::text NOT NULL,
    reminders_enabled boolean DEFAULT false NOT NULL,
    version bigint DEFAULT 1 NOT NULL,
    CONSTRAINT expense_settings_base_currency_check CHECK ((base_currency ~ '^[A-Z]{3}$'::text)),
    CONSTRAINT expense_settings_budget_units_check CHECK (((budget_units >= 0) AND (budget_units <= '9000000000000000'::bigint))),
    CONSTRAINT expense_settings_singleton_check CHECK (singleton)
);

CREATE TABLE public.expense_subscriptions (
    id bigint NOT NULL,
    name text NOT NULL,
    amount_units bigint NOT NULL,
    currency text NOT NULL,
    start_date date,
    next_payment date NOT NULL,
    anchor_payment date NOT NULL,
    cycle text NOT NULL,
    frequency integer NOT NULL,
    category_id bigint,
    payment_method_id bigint,
    payer_id bigint,
    notes text DEFAULT ''::text NOT NULL,
    url text DEFAULT ''::text NOT NULL,
    inactive boolean DEFAULT false NOT NULL,
    auto_renew boolean DEFAULT true NOT NULL,
    notify boolean DEFAULT false NOT NULL,
    notify_days_before integer DEFAULT 3 NOT NULL,
    cancellation_date date,
    replacement_id bigint,
    version bigint DEFAULT 1 NOT NULL,
    created_at bigint NOT NULL,
    updated_at bigint NOT NULL,
    CONSTRAINT expense_subscriptions_amount_units_check CHECK (((amount_units >= 0) AND (amount_units <= '9000000000000000'::bigint))),
    CONSTRAINT expense_subscriptions_check CHECK (((replacement_id IS NULL) OR (replacement_id <> id))),
    CONSTRAINT expense_subscriptions_check1 CHECK (((start_date IS NULL) OR (start_date <= next_payment))),
    CONSTRAINT expense_subscriptions_check2 CHECK (((cancellation_date IS NULL) OR (start_date IS NULL) OR (cancellation_date >= start_date))),
    CONSTRAINT expense_subscriptions_currency_check CHECK ((currency ~ '^[A-Z]{3}$'::text)),
    CONSTRAINT expense_subscriptions_cycle_check CHECK ((cycle = ANY (ARRAY['day'::text, 'week'::text, 'month'::text, 'year'::text]))),
    CONSTRAINT expense_subscriptions_frequency_check CHECK (((frequency >= 1) AND (frequency <= 1000))),
    CONSTRAINT expense_subscriptions_notify_days_before_check CHECK (((notify_days_before >= 0) AND (notify_days_before <= 365)))
);

CREATE SEQUENCE public.expense_subscriptions_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

ALTER SEQUENCE public.expense_subscriptions_id_seq OWNED BY public.expense_subscriptions.id;

CREATE TABLE public.fleet_config_history (
    id uuid NOT NULL,
    server_id bigint NOT NULL,
    path text NOT NULL,
    operation_id uuid NOT NULL,
    previous_sha256 text NOT NULL,
    sha256 text NOT NULL,
    content text NOT NULL,
    created_at bigint NOT NULL
);

CREATE TABLE public.fleet_cost_records (
    id uuid NOT NULL,
    server_id bigint NOT NULL,
    kind text NOT NULL,
    amount_minor bigint NOT NULL,
    currency text NOT NULL,
    occurred_at bigint NOT NULL,
    valid_until bigint,
    reference text NOT NULL,
    created_at bigint NOT NULL,
    CONSTRAINT fleet_cost_records_amount_minor_check CHECK ((amount_minor >= 0)),
    CONSTRAINT fleet_cost_records_kind_check CHECK ((kind = ANY (ARRAY['purchase'::text, 'renewal'::text, 'price_change'::text, 'refund'::text, 'cancellation'::text])))
);

CREATE TABLE public.fleet_events (
    id uuid NOT NULL,
    server_id bigint,
    kind text NOT NULL,
    source text NOT NULL,
    occurred_at bigint NOT NULL,
    detail jsonb NOT NULL
);

CREATE TABLE public.fleet_operations (
    id uuid NOT NULL,
    server_id bigint NOT NULL,
    operation jsonb NOT NULL,
    policy jsonb NOT NULL,
    requested_by bigint,
    automation_job_id uuid,
    requested_at bigint NOT NULL,
    expires_at bigint NOT NULL,
    status text NOT NULL,
    dispatched_at bigint,
    result jsonb,
    result_digest text,
    reconciliation_of uuid,
    reconciled_at bigint,
    reconciliation jsonb,
    CONSTRAINT fleet_operations_status_check CHECK ((status = ANY (ARRAY['queued'::text, 'dispatched'::text, 'succeeded'::text, 'failed'::text, 'expired'::text, 'unknown'::text, 'cancelled'::text])))
);

CREATE TABLE public.fleet_profiles (
    server_id bigint NOT NULL,
    policy jsonb DEFAULT '{}'::jsonb NOT NULL,
    lifecycle text DEFAULT 'active'::text NOT NULL,
    maintenance_from bigint,
    maintenance_until bigint,
    maintenance_reason text DEFAULT ''::text NOT NULL,
    updated_at bigint DEFAULT 0 NOT NULL,
    CONSTRAINT fleet_profiles_lifecycle_check CHECK ((lifecycle = ANY (ARRAY['active'::text, 'maintenance'::text, 'draining'::text, 'retired'::text])))
);

CREATE TABLE public.fleet_templates (
    id uuid NOT NULL,
    name text NOT NULL,
    config jsonb NOT NULL,
    created_at bigint NOT NULL,
    updated_at bigint NOT NULL
);

CREATE TABLE public.fleet_terminal_inputs (
    session_id uuid NOT NULL,
    sequence bigint NOT NULL,
    data text NOT NULL,
    columns integer,
    rows integer
);

CREATE TABLE public.fleet_terminal_outputs (
    session_id uuid NOT NULL,
    sequence bigint NOT NULL,
    data text NOT NULL,
    digest text NOT NULL,
    created_at bigint NOT NULL
);

CREATE TABLE public.fleet_terminal_sessions (
    id uuid NOT NULL,
    server_id bigint NOT NULL,
    admin_id bigint NOT NULL,
    account text NOT NULL,
    admin_session_hash text NOT NULL,
    policy jsonb NOT NULL,
    columns integer NOT NULL,
    rows integer NOT NULL,
    created_at bigint NOT NULL,
    expires_at bigint NOT NULL,
    last_input_at bigint NOT NULL,
    status text DEFAULT 'queued'::text NOT NULL,
    close_requested boolean DEFAULT false NOT NULL,
    input_sequence bigint DEFAULT 0 NOT NULL,
    output_sequence bigint DEFAULT 0 NOT NULL,
    output_bytes bigint DEFAULT 0 NOT NULL,
    error text
);

CREATE TABLE public.latency_tasks (
    id uuid NOT NULL,
    spec jsonb NOT NULL,
    default_enabled boolean DEFAULT false NOT NULL,
    revision bigint DEFAULT 1 NOT NULL
);

CREATE TABLE public.management_api_tokens (
    id uuid NOT NULL,
    admin_id bigint NOT NULL,
    token_hash text NOT NULL,
    name text NOT NULL,
    capabilities jsonb NOT NULL,
    server_ids jsonb NOT NULL,
    all_servers boolean DEFAULT false NOT NULL,
    expires_at bigint NOT NULL,
    revoked_at bigint,
    created_at bigint NOT NULL,
    last_used_at bigint
);

CREATE TABLE public.management_audit (
    id bigint NOT NULL,
    admin_id bigint,
    token_id uuid,
    action text NOT NULL,
    object_path text NOT NULL,
    request_diff jsonb NOT NULL,
    result jsonb NOT NULL,
    occurred_at bigint NOT NULL
);

CREATE SEQUENCE public.management_audit_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

ALTER SEQUENCE public.management_audit_id_seq OWNED BY public.management_audit.id;

CREATE TABLE public.metrics_minutely (
    server_id bigint NOT NULL,
    bucket bigint NOT NULL,
    metrics jsonb NOT NULL,
    sampled_at bigint DEFAULT 0 NOT NULL
);

CREATE TABLE public.network_acme_jobs (
    id uuid NOT NULL,
    certificate_id uuid NOT NULL,
    plan_revision bigint NOT NULL,
    requested_by bigint NOT NULL,
    certificate_revision bigint NOT NULL,
    domain_snapshot jsonb NOT NULL,
    request jsonb NOT NULL,
    status text NOT NULL,
    created_at bigint NOT NULL,
    started_at bigint,
    completed_at bigint,
    result jsonb,
    key_secret_ref uuid,
    version_id uuid
);

CREATE TABLE public.network_acme_plans (
    certificate_id uuid NOT NULL,
    config jsonb NOT NULL,
    revision bigint DEFAULT 1 NOT NULL,
    requested_by bigint NOT NULL,
    account_secret_ref uuid,
    next_run_at bigint NOT NULL,
    status text DEFAULT 'pending'::text NOT NULL,
    updated_at bigint NOT NULL
);

CREATE TABLE public.network_certificate_deployment_previews (
    id uuid NOT NULL,
    certificate_id uuid NOT NULL,
    certificate_revision bigint CONSTRAINT network_certificate_deployment_pr_certificate_revision_not_null NOT NULL,
    version_id uuid NOT NULL,
    key_secret_ref uuid NOT NULL,
    snapshot jsonb NOT NULL,
    adopt_existing boolean NOT NULL,
    created_at bigint NOT NULL,
    expires_at bigint NOT NULL,
    applied_at bigint
);

CREATE TABLE public.network_certificate_deployments (
    id uuid NOT NULL,
    certificate_id uuid NOT NULL,
    certificate_revision bigint NOT NULL,
    version_id uuid NOT NULL,
    key_secret_ref uuid NOT NULL,
    server_id bigint NOT NULL,
    target jsonb NOT NULL,
    adopt_existing boolean NOT NULL,
    requested_by bigint NOT NULL,
    operation_id uuid,
    status text NOT NULL,
    created_at bigint NOT NULL
);

CREATE TABLE public.network_certificate_versions (
    id uuid NOT NULL,
    certificate_id uuid NOT NULL,
    revision bigint NOT NULL,
    public_chain text NOT NULL,
    fingerprint text NOT NULL,
    not_before bigint NOT NULL,
    not_after bigint NOT NULL,
    created_at bigint NOT NULL
);

CREATE TABLE public.network_dns_challenges (
    id uuid NOT NULL,
    certificate_id uuid NOT NULL,
    ddns_rule_id uuid NOT NULL,
    credential_id uuid,
    name text NOT NULL,
    value text NOT NULL,
    record_id text,
    status text NOT NULL,
    error_code text,
    expires_at bigint NOT NULL,
    created_at bigint NOT NULL,
    updated_at bigint NOT NULL
);

CREATE TABLE public.network_document_history (
    id uuid NOT NULL,
    document_id uuid NOT NULL,
    kind text NOT NULL,
    revision bigint NOT NULL,
    config jsonb NOT NULL,
    action text NOT NULL,
    occurred_at bigint NOT NULL
);

CREATE TABLE public.network_documents (
    id uuid NOT NULL,
    kind text NOT NULL,
    revision bigint DEFAULT 1 NOT NULL,
    config jsonb NOT NULL,
    active_version uuid,
    created_at bigint NOT NULL,
    updated_at bigint NOT NULL,
    CONSTRAINT network_documents_kind_check CHECK ((kind = ANY (ARRAY['domain'::text, 'certificate'::text, 'endpoint'::text, 'forwarding'::text, 'tuning'::text, 'tunnel'::text, 'mesh'::text, 'firewall'::text])))
);

CREATE TABLE public.network_observations (
    id uuid NOT NULL,
    document_id uuid NOT NULL,
    server_id bigint,
    source text NOT NULL,
    result jsonb NOT NULL,
    observed_at bigint NOT NULL
);

CREATE TABLE public.network_operation_links (
    operation_id uuid NOT NULL,
    document_id uuid,
    revision bigint,
    server_id bigint NOT NULL,
    request jsonb NOT NULL,
    created_at bigint NOT NULL
);

CREATE TABLE public.network_probes (
    id uuid NOT NULL,
    server_id bigint NOT NULL,
    spec jsonb NOT NULL,
    task_id uuid,
    revision bigint DEFAULT 1 NOT NULL,
    CONSTRAINT network_probes_revision_check CHECK ((revision > 0))
);

CREATE TABLE public.network_server_migration_previews (
    id uuid NOT NULL,
    requested_by bigint NOT NULL,
    source_server_id bigint NOT NULL,
    target_server_id bigint NOT NULL,
    request jsonb NOT NULL,
    identity_map jsonb NOT NULL,
    snapshot jsonb NOT NULL,
    snapshot_digest text NOT NULL,
    created_at bigint NOT NULL,
    expires_at bigint NOT NULL,
    applied_at bigint,
    result jsonb
);

CREATE TABLE public.network_workbench_ip_evidence (
    id uuid NOT NULL,
    server_id bigint,
    address text NOT NULL,
    family text NOT NULL,
    source text NOT NULL,
    status text NOT NULL,
    evidence jsonb NOT NULL,
    observed_at bigint NOT NULL,
    expires_at bigint NOT NULL
);

CREATE TABLE public.network_workbench_pairings (
    id uuid NOT NULL,
    run_id uuid,
    token_hash text NOT NULL,
    expires_at bigint NOT NULL,
    used_at bigint,
    definition jsonb NOT NULL,
    created_at bigint NOT NULL
);

CREATE TABLE public.network_workbench_plans (
    id uuid NOT NULL,
    name text NOT NULL,
    definition jsonb NOT NULL,
    revision bigint DEFAULT 1 NOT NULL,
    enabled boolean DEFAULT false NOT NULL,
    interval_secs bigint,
    next_run_at bigint,
    created_at bigint NOT NULL,
    updated_at bigint NOT NULL
);

CREATE TABLE public.network_workbench_providers (
    id uuid NOT NULL,
    name text NOT NULL,
    endpoint text NOT NULL,
    credential_ref text,
    daily_quota integer DEFAULT 100 NOT NULL,
    requests_today integer DEFAULT 0 NOT NULL,
    quota_day bigint DEFAULT 0 NOT NULL,
    cache_seconds integer DEFAULT 3600 NOT NULL,
    disabled boolean DEFAULT false NOT NULL,
    updated_at bigint NOT NULL
);

CREATE TABLE public.network_workbench_results (
    id uuid NOT NULL,
    run_id uuid NOT NULL,
    step_index integer NOT NULL,
    server_id bigint,
    role text DEFAULT 'source'::text NOT NULL,
    job_id uuid,
    status text NOT NULL,
    result jsonb,
    created_at bigint NOT NULL,
    updated_at bigint NOT NULL
);

CREATE TABLE public.network_workbench_runs (
    id uuid NOT NULL,
    plan_id uuid,
    snapshot jsonb NOT NULL,
    status text NOT NULL,
    current_step integer DEFAULT 0 NOT NULL,
    actor text NOT NULL,
    error text,
    cancel_requested_at bigint,
    created_at bigint NOT NULL,
    updated_at bigint NOT NULL,
    CONSTRAINT network_workbench_runs_status_check CHECK ((status = ANY (ARRAY['queued'::text, 'running'::text, 'paused'::text, 'cleaning'::text, 'succeeded'::text, 'failed'::text, 'cancel_requested'::text, 'cancelled'::text])))
);

CREATE TABLE public.network_workbench_shares (
    id uuid NOT NULL,
    run_id uuid NOT NULL,
    token_hash text NOT NULL,
    expires_at bigint NOT NULL,
    revoked_at bigint,
    redaction jsonb NOT NULL,
    created_at bigint NOT NULL
);

CREATE TABLE public.network_workbench_targets (
    id uuid NOT NULL,
    name text NOT NULL,
    host text NOT NULL,
    region text DEFAULT ''::text NOT NULL,
    carrier text DEFAULT ''::text NOT NULL,
    purpose text NOT NULL,
    authorization_snapshot text NOT NULL,
    authorized_until bigint,
    created_at bigint NOT NULL,
    updated_at bigint NOT NULL
);

CREATE TABLE public.network_workbench_tools (
    id text NOT NULL,
    version text NOT NULL,
    license text NOT NULL,
    source_url text NOT NULL,
    signature_required boolean DEFAULT true NOT NULL,
    licensed boolean DEFAULT false NOT NULL,
    platforms text[] DEFAULT '{}'::text[] NOT NULL,
    updated_at bigint NOT NULL
);

CREATE TABLE public.nodes (
    id bigint NOT NULL,
    name text NOT NULL,
    server_id bigint NOT NULL,
    protocol text DEFAULT 'vless-reality'::text NOT NULL,
    port integer NOT NULL,
    public_host text NOT NULL,
    sni text NOT NULL,
    private_key text NOT NULL,
    public_key text NOT NULL,
    short_id text NOT NULL,
    deleted_at bigint,
    protocol_config jsonb DEFAULT '{"type": "vless-reality"}'::jsonb NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    settings jsonb DEFAULT '{}'::jsonb NOT NULL,
    resource_revision bigint DEFAULT 1 NOT NULL,
    CONSTRAINT nodes_port_check CHECK (((port >= 1) AND (port <= 65535))),
    CONSTRAINT nodes_protocol_check CHECK ((protocol = ANY (ARRAY['vless-reality'::text, 'vless-tunnel'::text, 'hysteria2'::text, 'shadowsocks2022'::text, 'tuic'::text, 'anytls'::text, 'naive'::text, 'snell-v6'::text, 'vless-tls'::text, 'trojan'::text, 'vmess'::text, 'http'::text, 'mixed'::text, 'socks'::text, 'shadowsocks'::text, 'shadowtls'::text]))),
    CONSTRAINT nodes_protocol_config_check CHECK (((jsonb_typeof(protocol_config) = 'object'::text) AND (protocol_config ? 'type'::text) AND COALESCE(((protocol_config ->> 'type'::text) = protocol), false))),
    CONSTRAINT nodes_resource_revision_check CHECK ((resource_revision > 0)),
    CONSTRAINT nodes_settings_check CHECK ((jsonb_typeof(settings) = 'object'::text))
);

CREATE SEQUENCE public.nodes_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

ALTER SEQUENCE public.nodes_id_seq OWNED BY public.nodes.id;

CREATE TABLE public.notification_channel_tests (
    channel text NOT NULL,
    attempted_at bigint NOT NULL,
    success boolean,
    last_error text,
    CONSTRAINT notification_channel_tests_channel_check CHECK ((channel = ANY (ARRAY['telegram'::text, 'webhook'::text])))
);

CREATE TABLE public.notification_outbox (
    id bigint NOT NULL,
    event_id bigint NOT NULL,
    kind text NOT NULL,
    message text NOT NULL,
    status text DEFAULT 'pending'::text NOT NULL,
    attempts integer DEFAULT 0 NOT NULL,
    next_attempt_at bigint NOT NULL,
    last_error text,
    channel text DEFAULT 'telegram'::text NOT NULL,
    last_attempt_at bigint,
    delivered_at bigint,
    CONSTRAINT notification_outbox_channel_check CHECK ((channel = ANY (ARRAY['telegram'::text, 'webhook'::text]))),
    CONSTRAINT notification_outbox_kind_check CHECK ((kind = ANY (ARRAY['offline'::text, 'alert'::text, 'recovery'::text]))),
    CONSTRAINT notification_outbox_status_check CHECK ((status = ANY (ARRAY['pending'::text, 'sent'::text, 'failed'::text, 'cancelled'::text])))
);

CREATE SEQUENCE public.notification_outbox_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

ALTER SEQUENCE public.notification_outbox_id_seq OWNED BY public.notification_outbox.id;

CREATE TABLE public.notification_test_limit (
    singleton boolean DEFAULT true NOT NULL,
    sent_at bigint DEFAULT 0 NOT NULL,
    CONSTRAINT notification_test_limit_singleton_check CHECK (singleton)
);

CREATE TABLE public.operations_backup_records (
    id uuid NOT NULL,
    name text NOT NULL,
    manifest jsonb NOT NULL,
    manifest_sha256 text NOT NULL,
    storage_sha256 text,
    location text NOT NULL,
    encrypted boolean NOT NULL,
    verification text NOT NULL,
    created_at bigint NOT NULL,
    imported_at bigint NOT NULL,
    imported_by bigint NOT NULL,
    source_schedule uuid,
    retain_until bigint,
    dependency_ids uuid[] DEFAULT '{}'::uuid[] NOT NULL,
    retired_at bigint,
    CONSTRAINT operations_backup_records_verification_check CHECK ((verification = ANY (ARRAY['declared'::text, 'integrity_verified'::text, 'restored'::text])))
);

CREATE TABLE public.operations_backup_schedules (
    id uuid NOT NULL,
    name text NOT NULL,
    requested_by bigint NOT NULL,
    interval_secs bigint NOT NULL,
    next_run_at bigint NOT NULL,
    paused boolean DEFAULT false NOT NULL,
    recipient text NOT NULL,
    retention_count integer NOT NULL,
    retention_days integer NOT NULL,
    last_started_at bigint,
    last_finished_at bigint,
    last_error text,
    claimed_id uuid,
    created_at bigint NOT NULL,
    updated_at bigint NOT NULL,
    CONSTRAINT operations_backup_schedules_interval_secs_check CHECK (((interval_secs >= 3600) AND (interval_secs <= 31536000))),
    CONSTRAINT operations_backup_schedules_retention_count_check CHECK (((retention_count >= 1) AND (retention_count <= 1000))),
    CONSTRAINT operations_backup_schedules_retention_days_check CHECK (((retention_days >= 1) AND (retention_days <= 3650)))
);

CREATE TABLE public.operations_cancellation_reminders (
    record_id uuid NOT NULL,
    enabled boolean DEFAULT false NOT NULL,
    lead_secs bigint DEFAULT 604800 NOT NULL,
    configured_by bigint NOT NULL,
    configured_at bigint NOT NULL,
    notified_at bigint,
    incident_id uuid,
    last_error text,
    CONSTRAINT operations_cancellation_reminders_lead_secs_check CHECK (((lead_secs >= 3600) AND (lead_secs <= 2592000)))
);

CREATE TABLE public.operations_cloud_links (
    resource_id uuid NOT NULL,
    server_id bigint,
    notes text DEFAULT ''::text NOT NULL,
    updated_at bigint NOT NULL,
    updated_by bigint NOT NULL
);

CREATE TABLE public.operations_cloud_observations (
    id bigint NOT NULL,
    resource_id uuid NOT NULL,
    checked_at bigint NOT NULL,
    source text NOT NULL,
    snapshot jsonb NOT NULL,
    previous_snapshot jsonb,
    changes jsonb NOT NULL
);

CREATE SEQUENCE public.operations_cloud_observations_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

ALTER SEQUENCE public.operations_cloud_observations_id_seq OWNED BY public.operations_cloud_observations.id;

CREATE TABLE public.operations_hetzner_accounts (
    id uuid NOT NULL,
    name text NOT NULL,
    credential_id uuid NOT NULL,
    revision bigint DEFAULT 1 NOT NULL,
    archived boolean DEFAULT false NOT NULL,
    created_at bigint NOT NULL,
    updated_at bigint NOT NULL,
    updated_by bigint NOT NULL,
    last_attempt_at bigint,
    last_read_at bigint,
    last_error text,
    refresh_id uuid,
    refresh_started_at bigint,
    CONSTRAINT operations_hetzner_accounts_name_check CHECK (((octet_length(name) >= 1) AND (octet_length(name) <= 800))),
    CONSTRAINT operations_hetzner_accounts_revision_check CHECK ((revision > 0))
);

CREATE TABLE public.operations_hetzner_observations (
    id bigint NOT NULL,
    resource_id uuid NOT NULL,
    refresh_id uuid NOT NULL,
    observed_at bigint NOT NULL,
    source text NOT NULL,
    presence text NOT NULL,
    snapshot jsonb NOT NULL,
    changes jsonb NOT NULL,
    CONSTRAINT operations_hetzner_observations_presence_check CHECK ((presence = ANY (ARRAY['observed'::text, 'absent'::text, 'unknown'::text]))),
    CONSTRAINT operations_hetzner_observations_source_check CHECK ((source = 'hetzner_cloud_v1_servers'::text))
);

CREATE SEQUENCE public.operations_hetzner_observations_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

ALTER SEQUENCE public.operations_hetzner_observations_id_seq OWNED BY public.operations_hetzner_observations.id;

CREATE TABLE public.operations_hetzner_refreshes (
    id uuid NOT NULL,
    account_id uuid NOT NULL,
    started_at bigint NOT NULL,
    completed_at bigint NOT NULL,
    complete boolean NOT NULL,
    pages integer NOT NULL,
    observed integer NOT NULL,
    error_code text,
    CONSTRAINT operations_hetzner_refreshes_check CHECK ((complete = (error_code IS NULL))),
    CONSTRAINT operations_hetzner_refreshes_observed_check CHECK (((observed >= 0) AND (observed <= 500))),
    CONSTRAINT operations_hetzner_refreshes_pages_check CHECK (((pages >= 0) AND (pages <= 10)))
);

CREATE TABLE public.operations_hetzner_resources (
    id uuid NOT NULL,
    account_id uuid NOT NULL,
    cloud_id text NOT NULL,
    name text NOT NULL,
    snapshot jsonb NOT NULL,
    presence text NOT NULL,
    last_seen_at bigint,
    verified_at bigint,
    last_attempt_at bigint NOT NULL,
    error_code text,
    server_id bigint,
    notes text DEFAULT ''::text NOT NULL,
    updated_at bigint NOT NULL,
    updated_by bigint,
    CONSTRAINT operations_hetzner_resources_presence_check CHECK ((presence = ANY (ARRAY['observed'::text, 'absent'::text, 'unknown'::text])))
);

CREATE TABLE public.operations_history (
    id bigint NOT NULL,
    job_id uuid NOT NULL,
    actor bigint,
    action text NOT NULL,
    details jsonb NOT NULL,
    recorded_at bigint NOT NULL
);

CREATE SEQUENCE public.operations_history_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

ALTER SEQUENCE public.operations_history_id_seq OWNED BY public.operations_history.id;

CREATE TABLE public.operations_incident_deliveries (
    id bigint NOT NULL,
    incident_id uuid NOT NULL,
    channel text NOT NULL,
    kind text NOT NULL,
    payload text NOT NULL,
    status text NOT NULL,
    attempts integer DEFAULT 0 NOT NULL,
    next_attempt_at bigint NOT NULL,
    last_error text,
    delivered_at bigint,
    CONSTRAINT operations_incident_deliveries_channel_check CHECK ((channel = ANY (ARRAY['telegram'::text, 'webhook'::text]))),
    CONSTRAINT operations_incident_deliveries_kind_check CHECK ((kind = ANY (ARRAY['alert'::text, 'escalation'::text, 'recovery'::text]))),
    CONSTRAINT operations_incident_deliveries_status_check CHECK ((status = ANY (ARRAY['pending'::text, 'sent'::text, 'failed'::text, 'cancelled'::text])))
);

CREATE SEQUENCE public.operations_incident_deliveries_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

ALTER SEQUENCE public.operations_incident_deliveries_id_seq OWNED BY public.operations_incident_deliveries.id;

CREATE TABLE public.operations_incident_notes (
    id bigint NOT NULL,
    incident_id uuid NOT NULL,
    actor bigint,
    kind text NOT NULL,
    note text NOT NULL,
    evidence jsonb NOT NULL,
    created_at bigint NOT NULL
);

CREATE SEQUENCE public.operations_incident_notes_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

ALTER SEQUENCE public.operations_incident_notes_id_seq OWNED BY public.operations_incident_notes.id;

CREATE TABLE public.operations_incidents (
    id uuid NOT NULL,
    source_key text NOT NULL,
    server_id bigint,
    title text NOT NULL,
    severity text NOT NULL,
    status text NOT NULL,
    opened_at bigint NOT NULL,
    observed_at bigint NOT NULL,
    acknowledged_at bigint,
    acknowledged_by bigint,
    assignee bigint,
    escalation_after_secs integer DEFAULT 3600 NOT NULL,
    escalated_at bigint,
    resolved_at bigint,
    evidence jsonb NOT NULL,
    recovery_evidence jsonb,
    conclusion text,
    notification_event bigint,
    CONSTRAINT operations_incidents_escalation_after_secs_check CHECK (((escalation_after_secs >= 60) AND (escalation_after_secs <= 604800))),
    CONSTRAINT operations_incidents_severity_check CHECK ((severity = ANY (ARRAY['info'::text, 'warning'::text, 'critical'::text]))),
    CONSTRAINT operations_incidents_status_check CHECK ((status = ANY (ARRAY['open'::text, 'acknowledged'::text, 'resolved'::text])))
);

CREATE TABLE public.operations_jobs (
    id uuid NOT NULL,
    name text NOT NULL,
    requested_by bigint NOT NULL,
    template_id uuid,
    spec jsonb NOT NULL,
    targets bigint[] NOT NULL,
    status text NOT NULL,
    created_at bigint NOT NULL,
    updated_at bigint NOT NULL,
    expires_at bigint NOT NULL,
    preview_digest text NOT NULL,
    failure_reason text,
    cancel_requested_at bigint,
    approved_batch integer DEFAULT 0 NOT NULL,
    source_schedule uuid,
    source_window bigint,
    CONSTRAINT operations_jobs_status_check CHECK ((status = ANY (ARRAY['preview'::text, 'queued'::text, 'running'::text, 'paused'::text, 'cancel_requested'::text, 'succeeded'::text, 'failed'::text, 'cancelled'::text, 'uncertain'::text]))),
    CONSTRAINT operations_jobs_targets_check CHECK (((cardinality(targets) >= 1) AND (cardinality(targets) <= 256)))
);

CREATE TABLE public.operations_maintenance (
    id uuid NOT NULL,
    name text NOT NULL,
    targets bigint[] NOT NULL,
    starts_at bigint NOT NULL,
    ends_at bigint NOT NULL,
    suppress_notifications boolean DEFAULT true NOT NULL,
    block_new_tasks boolean DEFAULT true NOT NULL,
    created_by bigint NOT NULL,
    created_at bigint NOT NULL,
    CONSTRAINT operations_maintenance_check CHECK ((ends_at > starts_at)),
    CONSTRAINT operations_maintenance_targets_check CHECK (((cardinality(targets) >= 1) AND (cardinality(targets) <= 256)))
);

CREATE TABLE public.operations_panel_steps (
    job_id uuid NOT NULL,
    "position" integer NOT NULL,
    state text DEFAULT 'pending'::text NOT NULL,
    execution_id uuid NOT NULL,
    backup_id uuid,
    spec jsonb NOT NULL,
    claimed_at bigint,
    finished_at bigint,
    result jsonb,
    CONSTRAINT operations_panel_steps_position_check CHECK (("position" = 0)),
    CONSTRAINT operations_panel_steps_state_check CHECK ((state = ANY (ARRAY['pending'::text, 'queued'::text, 'running'::text, 'succeeded'::text, 'failed'::text, 'cancelled'::text, 'uncertain'::text])))
);

CREATE TABLE public.operations_remediation_rules (
    id uuid NOT NULL,
    name text NOT NULL,
    requested_by bigint NOT NULL,
    source_key text NOT NULL,
    plan jsonb NOT NULL,
    targets bigint[] NOT NULL,
    cooldown_secs bigint NOT NULL,
    max_runs integer NOT NULL,
    run_count integer DEFAULT 0 NOT NULL,
    paused boolean DEFAULT false NOT NULL,
    last_run_at bigint,
    last_job uuid,
    created_at bigint NOT NULL,
    updated_at bigint NOT NULL,
    CONSTRAINT operations_remediation_rules_cooldown_secs_check CHECK (((cooldown_secs >= 300) AND (cooldown_secs <= 604800))),
    CONSTRAINT operations_remediation_rules_max_runs_check CHECK (((max_runs >= 1) AND (max_runs <= 100)))
);

CREATE TABLE public.operations_restore_drills (
    id uuid NOT NULL,
    backup_id uuid NOT NULL,
    report jsonb NOT NULL,
    report_sha256 text NOT NULL,
    passed boolean NOT NULL,
    recorded_at bigint NOT NULL,
    recorded_by bigint NOT NULL
);

CREATE TABLE public.operations_schedules (
    id uuid NOT NULL,
    name text NOT NULL,
    requested_by bigint NOT NULL,
    spec jsonb NOT NULL,
    targets bigint[] NOT NULL,
    next_run_at bigint NOT NULL,
    interval_secs bigint NOT NULL,
    timezone_offset_minutes integer NOT NULL,
    missed_policy text NOT NULL,
    max_runs integer NOT NULL,
    run_count integer DEFAULT 0 NOT NULL,
    paused boolean DEFAULT true NOT NULL,
    last_job uuid,
    created_at bigint NOT NULL,
    updated_at bigint NOT NULL,
    CONSTRAINT operations_schedules_interval_secs_check CHECK (((interval_secs >= 300) AND (interval_secs <= 31536000))),
    CONSTRAINT operations_schedules_max_runs_check CHECK (((max_runs >= 1) AND (max_runs <= 10000))),
    CONSTRAINT operations_schedules_missed_policy_check CHECK ((missed_policy = ANY (ARRAY['skip'::text, 'run_once'::text]))),
    CONSTRAINT operations_schedules_targets_check CHECK (((cardinality(targets) >= 1) AND (cardinality(targets) <= 256))),
    CONSTRAINT operations_schedules_timezone_offset_minutes_check CHECK (((timezone_offset_minutes >= '-720'::integer) AND (timezone_offset_minutes <= 840)))
);

CREATE TABLE public.operations_server_locks (
    server_id bigint NOT NULL,
    job_id uuid NOT NULL,
    acquired_at bigint NOT NULL
);

CREATE TABLE public.operations_target_steps (
    job_id uuid NOT NULL,
    server_id bigint NOT NULL,
    "position" integer NOT NULL,
    batch integer NOT NULL,
    state text DEFAULT 'pending'::text NOT NULL,
    fleet_operation_id uuid,
    started_at bigint,
    finished_at bigint,
    result jsonb,
    runtime_operation_id uuid,
    CONSTRAINT operations_target_steps_state_check CHECK ((state = ANY (ARRAY['pending'::text, 'queued'::text, 'running'::text, 'succeeded'::text, 'failed'::text, 'cancel_requested'::text, 'cancelled'::text, 'skipped'::text, 'uncertain'::text])))
);

CREATE TABLE public.operations_templates (
    id uuid NOT NULL,
    name text NOT NULL,
    revision bigint DEFAULT 1 NOT NULL,
    steps jsonb NOT NULL,
    created_by bigint NOT NULL,
    created_at bigint NOT NULL,
    archived boolean DEFAULT false NOT NULL
);

CREATE TABLE public.panel_settings (
    singleton boolean DEFAULT true NOT NULL,
    settings jsonb DEFAULT '{}'::jsonb NOT NULL,
    CONSTRAINT panel_settings_singleton_check CHECK (singleton)
);

CREATE TABLE public.passkey_accounts (
    id uuid NOT NULL,
    generation bigint DEFAULT 0 NOT NULL
);

CREATE TABLE public.passkey_ceremonies (
    id uuid NOT NULL,
    account_id uuid NOT NULL,
    generation bigint NOT NULL,
    purpose text NOT NULL,
    binding_hash text NOT NULL,
    authorization_binding text NOT NULL,
    state jsonb NOT NULL,
    expires_at bigint NOT NULL
);

CREATE TABLE public.passkey_credentials (
    id uuid NOT NULL,
    account_id uuid NOT NULL,
    credential_id bytea NOT NULL,
    payload jsonb NOT NULL,
    signature_counter bigint DEFAULT 0 NOT NULL,
    name text NOT NULL,
    created_at bigint NOT NULL,
    last_used_at bigint,
    CONSTRAINT passkey_credentials_credential_id_check CHECK (((octet_length(credential_id) >= 1) AND (octet_length(credential_id) <= 1024))),
    CONSTRAINT passkey_credentials_name_check CHECK (((octet_length(name) >= 1) AND (octet_length(name) <= 256))),
    CONSTRAINT passkey_credentials_signature_counter_check CHECK (((signature_counter >= 0) AND (signature_counter <= '4294967295'::bigint)))
);

CREATE TABLE public.probe_leases (
    id uuid NOT NULL,
    server_id bigint NOT NULL,
    session_hash text NOT NULL,
    revision bigint NOT NULL,
    issued_at bigint NOT NULL,
    expires_at bigint NOT NULL,
    probe_digests jsonb NOT NULL,
    CONSTRAINT probe_leases_check CHECK (((expires_at > issued_at) AND ((expires_at - issued_at) <= 90))),
    CONSTRAINT probe_leases_issued_at_check CHECK ((issued_at > 0)),
    CONSTRAINT probe_leases_probe_digests_check CHECK ((jsonb_typeof(probe_digests) = 'object'::text)),
    CONSTRAINT probe_leases_revision_check CHECK ((revision >= 0))
);

CREATE TABLE public.probe_results (
    id uuid NOT NULL,
    server_id bigint NOT NULL,
    probe_id uuid NOT NULL,
    sampled_at bigint NOT NULL,
    result jsonb NOT NULL,
    digest text NOT NULL
);

CREATE TABLE public.record_retention_policy (
    kind text NOT NULL,
    days integer NOT NULL,
    updated_at bigint NOT NULL,
    CONSTRAINT record_retention_policy_days_check CHECK (((days >= 1) AND (days <= 3650))),
    CONSTRAINT record_retention_policy_kind_check CHECK ((kind = ANY (ARRAY['monitoring'::text, 'logs'::text, 'ip'::text, 'terminal'::text, 'proxy-access'::text, 'audit'::text])))
);

CREATE TABLE public.remote_commands (
    id uuid NOT NULL,
    server_id bigint NOT NULL,
    requested_at bigint NOT NULL,
    spec jsonb NOT NULL,
    result jsonb,
    result_digest text,
    state text DEFAULT 'queued'::text NOT NULL,
    lifecycle_version integer DEFAULT 0 NOT NULL,
    claim_id uuid,
    claimed_at bigint,
    started_at bigint,
    cancel_requested_at bigint,
    finished_at bigint,
    cancel_supported boolean DEFAULT false NOT NULL,
    CONSTRAINT remote_commands_state_check CHECK ((state = ANY (ARRAY['queued'::text, 'claimed'::text, 'running'::text, 'cancel_requested'::text, 'cancelled'::text, 'succeeded'::text, 'failed'::text, 'expired'::text, 'interrupted'::text])))
);

CREATE TABLE public.runtime_apply_facts (
    server_id bigint NOT NULL,
    op_id uuid NOT NULL,
    module text NOT NULL,
    rev bigint NOT NULL,
    result_json jsonb NOT NULL,
    received_at bigint NOT NULL
);

CREATE TABLE public.runtime_control_devices (
    server_id bigint NOT NULL,
    checkpoint_required boolean DEFAULT false NOT NULL
);

CREATE TABLE public.runtime_control_receipts (
    request_id uuid NOT NULL,
    result_json jsonb NOT NULL,
    result_sha256 text NOT NULL,
    received_at bigint NOT NULL,
    outcome text NOT NULL,
    CONSTRAINT runtime_control_receipts_outcome_check CHECK ((outcome = ANY (ARRAY['verified'::text, 'late'::text, 'superseded'::text, 'mismatch'::text, 'failed'::text]))),
    CONSTRAINT runtime_control_receipts_result_sha256_check CHECK ((result_sha256 ~ '^[0-9a-f]{64}$'::text))
);

CREATE TABLE public.runtime_control_requests (
    request_id uuid NOT NULL,
    server_id bigint NOT NULL,
    module text NOT NULL,
    kind text NOT NULL,
    request_digest text NOT NULL,
    request_json jsonb NOT NULL,
    created_at bigint NOT NULL,
    expires_at bigint NOT NULL,
    state text DEFAULT 'pending'::text NOT NULL,
    CONSTRAINT runtime_control_requests_check CHECK (((expires_at > created_at) AND (expires_at <= (created_at + 600)))),
    CONSTRAINT runtime_control_requests_kind_check CHECK ((kind = ANY (ARRAY['checkpoint'::text, 'barrier'::text, 'probe'::text]))),
    CONSTRAINT runtime_control_requests_request_digest_check CHECK ((request_digest ~ '^[0-9a-f]{64}$'::text)),
    CONSTRAINT runtime_control_requests_state_check CHECK ((state = ANY (ARRAY['pending'::text, 'received'::text, 'expired'::text])))
);

CREATE TABLE public.runtime_deployment_bindings (
    deployment_id uuid NOT NULL,
    server_id bigint NOT NULL,
    module text NOT NULL,
    rev bigint NOT NULL,
    bundle_sha256 text NOT NULL,
    binding_digest text NOT NULL,
    CONSTRAINT runtime_deployment_bindings_binding_digest_check CHECK ((binding_digest ~ '^[0-9a-f]{64}$'::text)),
    CONSTRAINT runtime_deployment_bindings_bundle_sha256_check CHECK ((bundle_sha256 ~ '^[0-9a-f]{64}$'::text)),
    CONSTRAINT runtime_deployment_bindings_rev_check CHECK ((rev > 0))
);

CREATE TABLE public.runtime_enablements (
    server_id bigint NOT NULL,
    module text NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    source text NOT NULL,
    enabled_at bigint NOT NULL,
    CONSTRAINT runtime_enablements_source_check CHECK ((source = ANY (ARRAY['administrator'::text, 'agent_capability'::text, 'legacy_nodes'::text, 'legacy_deployments'::text])))
);

CREATE TABLE public.runtime_module_checkpoints (
    server_id bigint NOT NULL,
    module text NOT NULL,
    checkpoint_json jsonb NOT NULL,
    checkpoint_request_id uuid NOT NULL,
    verified_at bigint NOT NULL,
    minimum_revision bigint DEFAULT 0 NOT NULL,
    barrier_request_id uuid,
    CONSTRAINT runtime_module_checkpoints_minimum_revision_check CHECK ((minimum_revision >= 0))
);

CREATE TABLE public.runtime_operations (
    id uuid NOT NULL,
    server_id bigint NOT NULL,
    module text NOT NULL,
    requested_at bigint NOT NULL,
    dispatched_at bigint,
    spec jsonb NOT NULL,
    result jsonb,
    automation_job_id uuid,
    cancelled_at bigint,
    reconciled_at bigint,
    reconciliation jsonb,
    CONSTRAINT runtime_operations_local_cancel CHECK (((cancelled_at IS NULL) OR (dispatched_at IS NULL))),
    CONSTRAINT runtime_operations_module_check CHECK (((char_length(module) >= 1) AND (char_length(module) <= 64))),
    CONSTRAINT runtime_operations_reconciliation CHECK (((reconciled_at IS NULL) = (reconciliation IS NULL)))
);

CREATE TABLE public.runtime_validations (
    id uuid NOT NULL,
    server_id bigint NOT NULL,
    module text NOT NULL,
    scope text NOT NULL,
    generation bigint NOT NULL,
    operation text NOT NULL,
    revision bigint NOT NULL,
    config_hash text NOT NULL,
    expires_at bigint NOT NULL,
    requested_at bigint NOT NULL,
    result jsonb,
    result_digest text,
    CONSTRAINT runtime_validations_generation_check CHECK ((generation > 0)),
    CONSTRAINT runtime_validations_operation_check CHECK ((operation = ANY (ARRAY['probe'::text, 'barrier'::text]))),
    CONSTRAINT runtime_validations_revision_check CHECK ((revision > 0))
);

CREATE TABLE public.server_alert_events (
    id bigint CONSTRAINT server_offline_events_id_not_null NOT NULL,
    server_id bigint CONSTRAINT server_offline_events_server_id_not_null NOT NULL,
    server_name text CONSTRAINT server_offline_events_server_name_not_null NOT NULL,
    last_seen bigint,
    opened_at bigint CONSTRAINT server_offline_events_opened_at_not_null NOT NULL,
    resolved_at bigint,
    resolution text,
    category text DEFAULT 'offline'::text NOT NULL,
    source_key text DEFAULT 'offline'::text NOT NULL,
    details jsonb DEFAULT '{}'::jsonb NOT NULL,
    message text DEFAULT ''::text NOT NULL,
    CONSTRAINT server_alert_events_category_check CHECK ((category = ANY (ARRAY['offline'::text, 'resource'::text, 'expiry'::text, 'traffic'::text])))
);

CREATE TABLE public.server_ip_quality (
    server_id bigint NOT NULL,
    ip text NOT NULL,
    payload jsonb NOT NULL,
    checked_at bigint NOT NULL,
    provider text DEFAULT 'check-place'::text NOT NULL,
    last_attempt_at bigint,
    last_success_at bigint,
    fresh_until bigint,
    last_error jsonb DEFAULT '{}'::jsonb NOT NULL,
    source_job_created_at bigint,
    source_job_id uuid,
    source_section_revision bigint
);

CREATE TABLE public.server_ip_quality_datasets (
    server_id bigint NOT NULL,
    ip text NOT NULL,
    provider text NOT NULL,
    database text NOT NULL,
    checked_at bigint NOT NULL,
    last_attempt jsonb NOT NULL,
    last_attempt_at bigint,
    last_success_at bigint,
    fresh_until bigint,
    last_error jsonb,
    success_payload jsonb
);

CREATE TABLE public.server_ip_quality_node_generations (
    server_id bigint NOT NULL,
    ip_version text NOT NULL,
    job_created_at bigint NOT NULL,
    job_generation bigint NOT NULL,
    job_id uuid NOT NULL,
    section_revision bigint NOT NULL,
    egress_ip text,
    CONSTRAINT server_ip_quality_node_generations_ip_version_check CHECK ((ip_version = ANY (ARRAY['4'::text, '6'::text]))),
    CONSTRAINT server_ip_quality_node_generations_section_revision_check CHECK ((section_revision > 0))
);

CREATE TABLE public.server_module_status (
    server_id bigint NOT NULL,
    module text NOT NULL,
    target_rev bigint DEFAULT 0 NOT NULL,
    applied_rev bigint DEFAULT 0 NOT NULL,
    last_result_rev bigint DEFAULT 0 NOT NULL,
    healthy boolean DEFAULT false NOT NULL,
    last_error text,
    updated_at bigint DEFAULT 0 NOT NULL,
    CONSTRAINT server_module_status_applied_rev_check CHECK ((applied_rev >= 0)),
    CONSTRAINT server_module_status_last_result_rev_check CHECK ((last_result_rev >= 0)),
    CONSTRAINT server_module_status_target_rev_check CHECK ((target_rev >= 0))
);

CREATE TABLE public.server_network_counters (
    server_id bigint NOT NULL,
    checkpoint jsonb NOT NULL
);

CREATE TABLE public.server_network_daily (
    server_id bigint NOT NULL,
    day bigint NOT NULL,
    interface text NOT NULL,
    uploaded numeric(39,0) NOT NULL,
    downloaded numeric(39,0) NOT NULL,
    first_sample_at bigint NOT NULL,
    last_sample_at bigint NOT NULL,
    incomplete boolean DEFAULT false NOT NULL,
    CONSTRAINT server_network_daily_downloaded_check CHECK ((downloaded >= (0)::numeric)),
    CONSTRAINT server_network_daily_uploaded_check CHECK ((uploaded >= (0)::numeric))
);

CREATE SEQUENCE public.server_offline_events_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

ALTER SEQUENCE public.server_offline_events_id_seq OWNED BY public.server_alert_events.id;

CREATE TABLE public.server_retirements (
    server_id bigint NOT NULL,
    request_id uuid NOT NULL,
    status text NOT NULL,
    error text,
    requested_at bigint NOT NULL,
    completed_at bigint,
    CONSTRAINT server_retirements_status_check CHECK ((status = ANY (ARRAY['pending'::text, 'failed'::text, 'confirmed'::text, 'offline_unconfirmed'::text])))
);

CREATE TABLE public.server_traffic_corrections (
    id bigint NOT NULL,
    server_id bigint NOT NULL,
    cycle_start bigint NOT NULL,
    reset_day integer NOT NULL,
    network_interface text NOT NULL,
    uploaded_offset numeric(39,0) NOT NULL,
    downloaded_offset numeric(39,0) NOT NULL,
    reason text NOT NULL,
    created_at bigint NOT NULL,
    invalidated_at bigint
);

CREATE SEQUENCE public.server_traffic_corrections_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

ALTER SEQUENCE public.server_traffic_corrections_id_seq OWNED BY public.server_traffic_corrections.id;

CREATE TABLE public.servers (
    id bigint NOT NULL,
    name text NOT NULL,
    device_public_key text,
    static_info jsonb DEFAULT '{}'::jsonb NOT NULL,
    last_seen bigint,
    deleted_at bigint,
    capabilities jsonb DEFAULT '[]'::jsonb NOT NULL,
    agent_settings jsonb DEFAULT '{"auto_update": false, "discover_public_ips": true, "sample_interval_secs": 1, "upload_interval_secs": 3}'::jsonb NOT NULL,
    last_heartbeat_at bigint,
    asset_settings jsonb DEFAULT '{}'::jsonb NOT NULL,
    static_info_received_at bigint,
    last_contact_at bigint
);

CREATE SEQUENCE public.servers_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

ALTER SEQUENCE public.servers_id_seq OWNED BY public.servers.id;

CREATE TABLE public.sessions (
    token_hash text NOT NULL,
    admin_id bigint NOT NULL,
    expires_at bigint NOT NULL
);

CREATE TABLE public.singbox_account_challenges (
    id uuid NOT NULL,
    purpose text NOT NULL,
    user_id bigint,
    address text NOT NULL,
    old_address text,
    token_hash text NOT NULL,
    old_token_hash text,
    invitation_hash text,
    attempts integer DEFAULT 0 NOT NULL,
    created_at bigint NOT NULL,
    expires_at bigint NOT NULL,
    consumed_at bigint,
    CONSTRAINT singbox_account_challenges_purpose_check CHECK ((purpose = ANY (ARRAY['invite'::text, 'reset'::text, 'email'::text])))
);

CREATE TABLE public.singbox_account_limits (
    bucket text NOT NULL,
    starts_at bigint NOT NULL,
    attempts integer NOT NULL
);

CREATE TABLE public.singbox_chain_runtime_requirements (
    chain_id bigint NOT NULL,
    generation bigint NOT NULL,
    server_id bigint NOT NULL,
    runtime_version text NOT NULL,
    artifact_sha256 text NOT NULL,
    artifact jsonb NOT NULL,
    CONSTRAINT singbox_chain_runtime_requirements_artifact_sha256_check CHECK ((artifact_sha256 ~ '^[0-9a-f]{64}$'::text)),
    CONSTRAINT singbox_chain_runtime_requirements_runtime_version_check CHECK ((runtime_version = ANY (ARRAY['1.14.2'::text, '1.14.2-apsp.1'::text, '1.14.0-apsp.1'::text, '1.14.4-apsp.1'::text, '1.14.4-apsp.2'::text])))
);

CREATE TABLE public.singbox_chain_state (
    chain_id bigint NOT NULL,
    available boolean NOT NULL
);

CREATE TABLE public.singbox_chains (
    id bigint NOT NULL,
    name text NOT NULL,
    entry_node_id bigint NOT NULL,
    path_kind text DEFAULT 'ordered'::text NOT NULL,
    minimum_generation bigint DEFAULT 0 NOT NULL,
    deleted_at bigint,
    settings_revision bigint DEFAULT 1 NOT NULL,
    desired_generation bigint DEFAULT 1 NOT NULL,
    applied_generation bigint,
    candidate_generation bigint,
    recovery_generation bigint,
    phase text DEFAULT 'preparing_dependencies'::text NOT NULL,
    last_error text,
    route_enabled boolean DEFAULT true NOT NULL,
    last_granted boolean DEFAULT false NOT NULL,
    restore_step text,
    CONSTRAINT singbox_chains_desired_generation_check CHECK ((desired_generation > 0)),
    CONSTRAINT singbox_chains_path_kind_check CHECK ((path_kind = 'ordered'::text)),
    CONSTRAINT singbox_chains_phase_check CHECK ((phase = ANY (ARRAY['preparing_dependencies'::text, 'preparing_entry'::text, 'probing_candidate'::text, 'switching_entry'::text, 'probing_switched'::text, 'fixing_barrier'::text, 'retiring_old'::text, 'applied'::text, 'restoring'::text, 'failed'::text, 'retiring'::text, 'retired'::text]))),
    CONSTRAINT singbox_chains_restore_step_check CHECK ((restore_step = ANY (ARRAY['entry'::text, 'probe'::text, 'barrier'::text, 'cleanup'::text]))),
    CONSTRAINT singbox_chains_settings_revision_check CHECK ((settings_revision > 0)),
    CONSTRAINT singbox_ordered_minimum_generation_check CHECK ((minimum_generation >= 0))
);

CREATE SEQUENCE public.singbox_chains_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

ALTER SEQUENCE public.singbox_chains_id_seq OWNED BY public.singbox_chains.id;

CREATE TABLE public.singbox_client_templates (
    user_id bigint NOT NULL,
    revision bigint DEFAULT 1 NOT NULL,
    client text NOT NULL,
    client_version text NOT NULL,
    definition jsonb NOT NULL,
    updated_at bigint NOT NULL,
    CONSTRAINT singbox_client_templates_client_check CHECK ((client = 'singbox'::text)),
    CONSTRAINT singbox_client_templates_client_version_check CHECK ((client_version = '1.14.4'::text)),
    CONSTRAINT singbox_client_templates_revision_check CHECK ((revision > 0))
);

CREATE TABLE public.singbox_credential_rotations (
    id uuid NOT NULL,
    user_id bigint NOT NULL,
    server_id bigint NOT NULL,
    baseline_revision bigint NOT NULL,
    node_ids bigint[] NOT NULL,
    credential_fingerprint text NOT NULL,
    requested_at bigint NOT NULL,
    confirmed_at bigint,
    confirmed_revision bigint
);

CREATE TABLE public.singbox_deployment_preflights (
    id uuid NOT NULL,
    sequence bigint NOT NULL,
    server_id bigint NOT NULL,
    administrator_id bigint NOT NULL,
    expected_digest text NOT NULL,
    snapshot jsonb NOT NULL,
    panel_checks jsonb NOT NULL,
    ports_operation_id uuid NOT NULL,
    permissions_operation_id uuid NOT NULL,
    created_at bigint NOT NULL,
    expires_at bigint NOT NULL,
    confirmed_at bigint,
    confirmed_by bigint,
    CONSTRAINT singbox_deployment_preflights_expected_digest_check CHECK ((length(expected_digest) = 64))
);

CREATE SEQUENCE public.singbox_deployment_preflights_sequence_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

ALTER SEQUENCE public.singbox_deployment_preflights_sequence_seq OWNED BY public.singbox_deployment_preflights.sequence;

CREATE TABLE public.singbox_deployment_projections (
    server_id bigint NOT NULL,
    rev bigint NOT NULL,
    source_json jsonb NOT NULL
);

CREATE TABLE public.singbox_deployment_public_projection (
    server_id bigint NOT NULL,
    module text DEFAULT 'singbox'::text NOT NULL,
    revision bigint NOT NULL,
    node_id bigint NOT NULL,
    public_fields jsonb NOT NULL,
    CONSTRAINT singbox_deployment_public_projection_public_fields_check CHECK ((jsonb_typeof(public_fields) = 'object'::text))
);

CREATE TABLE public.singbox_nexus_runtime_nodes (
    inventory_id bigint NOT NULL,
    node_id bigint NOT NULL,
    server_id bigint NOT NULL,
    legacy_group_ids bigint[] NOT NULL,
    rate numeric(12,6) NOT NULL,
    CONSTRAINT singbox_nexus_runtime_nodes_rate_check CHECK ((rate >= (0)::numeric))
);

CREATE TABLE public.singbox_ordered_chain_hops (
    chain_id bigint NOT NULL,
    generation bigint NOT NULL,
    "position" integer NOT NULL,
    kind text NOT NULL,
    endpoint_version_id uuid,
    managed_node_id bigint,
    managed_server_id bigint,
    relay_uuid uuid,
    source_id bigint,
    identity_epoch bigint,
    external_node_id uuid,
    node_version_id uuid,
    source_revision_id uuid,
    update_mode text,
    CONSTRAINT singbox_ordered_chain_hops_check CHECK ((((kind = 'managed'::text) AND (endpoint_version_id IS NOT NULL) AND (managed_node_id IS NOT NULL) AND (managed_server_id IS NOT NULL) AND (relay_uuid IS NOT NULL) AND (source_id IS NULL) AND (identity_epoch IS NULL) AND (external_node_id IS NULL) AND (node_version_id IS NULL) AND (source_revision_id IS NULL) AND (update_mode IS NULL)) OR ((kind = 'subscription'::text) AND (endpoint_version_id IS NULL) AND (managed_node_id IS NULL) AND (managed_server_id IS NULL) AND (relay_uuid IS NULL) AND (source_id IS NOT NULL) AND (identity_epoch > 0) AND (external_node_id IS NOT NULL) AND (node_version_id IS NOT NULL) AND (source_revision_id IS NOT NULL) AND (update_mode IS NOT NULL)))),
    CONSTRAINT singbox_ordered_chain_hops_kind_check CHECK ((kind = ANY (ARRAY['managed'::text, 'subscription'::text]))),
    CONSTRAINT singbox_ordered_chain_hops_position_check CHECK ((("position" >= 1) AND ("position" <= 8))),
    CONSTRAINT singbox_ordered_chain_hops_update_mode_check CHECK ((update_mode = ANY (ARRAY['follow_node'::text, 'pinned'::text])))
);

CREATE TABLE public.singbox_ordered_chain_versions (
    chain_id bigint NOT NULL,
    generation bigint NOT NULL,
    entry_endpoint_version uuid NOT NULL,
    semantic_sha256 text NOT NULL,
    capabilities jsonb NOT NULL,
    snapshot jsonb NOT NULL,
    created_at bigint NOT NULL,
    CONSTRAINT singbox_ordered_chain_versions_capabilities_check CHECK ((jsonb_typeof(capabilities) = 'object'::text)),
    CONSTRAINT singbox_ordered_chain_versions_generation_check CHECK ((generation > 0)),
    CONSTRAINT singbox_ordered_chain_versions_semantic_sha256_check CHECK ((semantic_sha256 ~ '^[0-9a-f]{64}$'::text)),
    CONSTRAINT singbox_ordered_chain_versions_snapshot_check CHECK ((jsonb_typeof(snapshot) = 'object'::text))
);

CREATE TABLE public.singbox_policy_chains (
    group_id bigint NOT NULL,
    chain_id bigint NOT NULL
);

CREATE TABLE public.singbox_policy_nodes (
    group_id bigint NOT NULL,
    node_id bigint NOT NULL
);

CREATE TABLE public.singbox_user_policies (
    user_id bigint NOT NULL,
    group_id bigint NOT NULL,
    manual_grant boolean DEFAULT true NOT NULL,
    commerce_grant boolean DEFAULT false NOT NULL
);

CREATE TABLE public.users (
    id bigint NOT NULL,
    name text NOT NULL,
    subscription_token text NOT NULL,
    deleted_at bigint
);

CREATE VIEW public.singbox_desired_accesses AS
 SELECT DISTINCT granted.user_id,
    granted.node_id
   FROM ((((( SELECT u_1.user_id,
            n_1.node_id
           FROM (public.singbox_user_policies u_1
             JOIN public.singbox_policy_nodes n_1 ON ((n_1.group_id = u_1.group_id)))
          WHERE ((NOT (EXISTS ( SELECT 1
                   FROM public.singbox_chains c_1
                  WHERE ((c_1.path_kind = 'ordered'::text) AND (c_1.entry_node_id = n_1.node_id) AND ((c_1.deleted_at IS NULL) OR (c_1.phase <> 'retired'::text)))))) AND (NOT (EXISTS ( SELECT 1
                   FROM public.singbox_nexus_runtime_nodes r
                  WHERE (r.node_id = n_1.node_id)))))
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

CREATE TABLE public.singbox_entitlement_state (
    user_id bigint NOT NULL,
    allowed boolean NOT NULL
);

CREATE SEQUENCE public.singbox_ordered_external_node_versions_public_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

CREATE SEQUENCE public.singbox_ordered_external_nodes_public_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

CREATE TABLE public.singbox_ordered_external_node_versions (
    id uuid NOT NULL,
    node_id uuid NOT NULL,
    source_revision_id uuid CONSTRAINT singbox_ordered_external_node_versi_source_revision_id_not_null NOT NULL,
    normalized_config jsonb,
    content_digest text,
    public_preview jsonb NOT NULL,
    capabilities jsonb NOT NULL,
    supported boolean NOT NULL,
    reasons jsonb NOT NULL,
    public_id bigint DEFAULT nextval('public.singbox_ordered_external_node_versions_public_id_seq'::regclass) NOT NULL,
    legacy_config jsonb,
    legacy_config_sha256 text,
    CONSTRAINT singbox_ordered_external_node_versio_legacy_config_sha256_check CHECK ((legacy_config_sha256 ~ '^[0-9a-f]{64}$'::text)),
    CONSTRAINT singbox_ordered_external_node_versions_capabilities_check CHECK ((jsonb_typeof(capabilities) = 'object'::text)),
    CONSTRAINT singbox_ordered_external_node_versions_check CHECK ((supported = ((normalized_config IS NOT NULL) AND (content_digest IS NOT NULL)))),
    CONSTRAINT singbox_ordered_external_node_versions_content_digest_check CHECK ((content_digest ~ '^[0-9a-f]{64}$'::text)),
    CONSTRAINT singbox_ordered_external_node_versions_legacy_config_check CHECK (((legacy_config IS NULL) OR (jsonb_typeof(legacy_config) = 'object'::text))),
    CONSTRAINT singbox_ordered_external_node_versions_public_preview_check CHECK ((jsonb_typeof(public_preview) = 'object'::text)),
    CONSTRAINT singbox_ordered_external_node_versions_reasons_check CHECK ((jsonb_typeof(reasons) = 'array'::text)),
    CONSTRAINT singbox_ordered_external_version_legacy CHECK (((legacy_config IS NULL) = (legacy_config_sha256 IS NULL)))
);

CREATE TABLE public.singbox_ordered_external_nodes (
    id uuid NOT NULL,
    source_id bigint NOT NULL,
    identity_epoch bigint NOT NULL,
    identity_key text,
    identity_state text NOT NULL,
    latest_version uuid,
    last_seen_revision uuid,
    created_at bigint NOT NULL,
    adopted boolean DEFAULT false NOT NULL,
    public_id bigint DEFAULT nextval('public.singbox_ordered_external_nodes_public_id_seq'::regclass) NOT NULL,
    CONSTRAINT singbox_ordered_external_nodes_check CHECK (((identity_state = 'unique'::text) = (identity_key IS NOT NULL))),
    CONSTRAINT singbox_ordered_external_nodes_identity_epoch_check CHECK ((identity_epoch > 0)),
    CONSTRAINT singbox_ordered_external_nodes_identity_state_check CHECK ((identity_state = ANY (ARRAY['unique'::text, 'ambiguous'::text, 'unresolved'::text])))
);

ALTER SEQUENCE public.singbox_ordered_external_node_versions_public_id_seq OWNED BY public.singbox_ordered_external_node_versions.public_id;

ALTER SEQUENCE public.singbox_ordered_external_nodes_public_id_seq OWNED BY public.singbox_ordered_external_nodes.public_id;

CREATE TABLE public.singbox_ordered_subscription_sources (
    id bigint NOT NULL,
    name text NOT NULL,
    kind text NOT NULL,
    host text,
    input_config jsonb NOT NULL,
    settings_revision bigint DEFAULT 1 NOT NULL,
    identity_epoch bigint DEFAULT 1 NOT NULL,
    archived boolean DEFAULT false NOT NULL,
    deleted_at bigint,
    refresh_interval_secs bigint CONSTRAINT singbox_ordered_subscription_sou_refresh_interval_secs_not_null NOT NULL,
    next_refresh_at bigint,
    last_attempt_at bigint,
    last_success_at bigint,
    current_success_revision uuid,
    last_error jsonb,
    conditional_etag text,
    conditional_last_modified text,
    conditional_settings_revision bigint,
    conditional_identity_epoch bigint,
    created_at bigint NOT NULL,
    updated_at bigint NOT NULL,
    user_agent text,
    auto_refresh boolean DEFAULT true NOT NULL,
    traffic jsonb DEFAULT '{}'::jsonb NOT NULL,
    changes jsonb DEFAULT '{"added": 0, "missing": 0, "updated": 0, "unsupported": 0}'::jsonb NOT NULL,
    CONSTRAINT singbox_ordered_source_refresh_interval CHECK ((((kind = 'inline'::text) AND (refresh_interval_secs = 0)) OR ((kind = 'url'::text) AND ((refresh_interval_secs >= 300) AND (refresh_interval_secs <= 2592000))))),
    CONSTRAINT singbox_ordered_subscription_sources_changes_check CHECK ((jsonb_typeof(changes) = 'object'::text)),
    CONSTRAINT singbox_ordered_subscription_sources_identity_epoch_check CHECK ((identity_epoch > 0)),
    CONSTRAINT singbox_ordered_subscription_sources_input_config_check CHECK ((jsonb_typeof(input_config) = 'object'::text)),
    CONSTRAINT singbox_ordered_subscription_sources_kind_check CHECK ((kind = ANY (ARRAY['url'::text, 'inline'::text]))),
    CONSTRAINT singbox_ordered_subscription_sources_settings_revision_check CHECK ((settings_revision > 0)),
    CONSTRAINT singbox_ordered_subscription_sources_traffic_check CHECK ((jsonb_typeof(traffic) = 'object'::text)),
    CONSTRAINT singbox_ordered_subscription_sources_user_agent_check CHECK (((user_agent IS NULL) OR ((octet_length(user_agent) >= 1) AND (octet_length(user_agent) <= 256))))
);

CREATE TABLE public.singbox_subscription_revision_nodes (
    source_revision_id uuid NOT NULL,
    ordinal integer NOT NULL,
    node_id uuid NOT NULL,
    version_id uuid NOT NULL,
    public_preview jsonb NOT NULL,
    identity_state text NOT NULL,
    CONSTRAINT singbox_subscription_revision_nodes_identity_state_check CHECK ((identity_state = ANY (ARRAY['unique'::text, 'ambiguous'::text, 'unresolved'::text]))),
    CONSTRAINT singbox_subscription_revision_nodes_ordinal_check CHECK (((ordinal >= 0) AND (ordinal < 5000))),
    CONSTRAINT singbox_subscription_revision_nodes_public_preview_check CHECK ((jsonb_typeof(public_preview) = 'object'::text))
);

CREATE TABLE public.singbox_subscription_source_revisions (
    id uuid NOT NULL,
    generation bigint NOT NULL,
    source_id bigint NOT NULL,
    job_id uuid NOT NULL,
    settings_revision bigint CONSTRAINT singbox_subscription_source_revision_settings_revision_not_null NOT NULL,
    identity_epoch bigint NOT NULL,
    parser_version text NOT NULL,
    format text NOT NULL,
    raw_digest text NOT NULL,
    parsed_at bigint NOT NULL,
    counts jsonb NOT NULL,
    warnings jsonb NOT NULL,
    CONSTRAINT singbox_subscription_source_revisions_counts_check CHECK ((jsonb_typeof(counts) = 'object'::text)),
    CONSTRAINT singbox_subscription_source_revisions_format_check CHECK ((format = ANY (ARRAY['uri_list'::text, 'base64_uri_list'::text, 'sing_box_json'::text, 'clash_yaml'::text]))),
    CONSTRAINT singbox_subscription_source_revisions_identity_epoch_check CHECK ((identity_epoch > 0)),
    CONSTRAINT singbox_subscription_source_revisions_raw_digest_check CHECK ((raw_digest ~ '^[0-9a-f]{64}$'::text)),
    CONSTRAINT singbox_subscription_source_revisions_settings_revision_check CHECK ((settings_revision > 0)),
    CONSTRAINT singbox_subscription_source_revisions_warnings_check CHECK ((jsonb_typeof(warnings) = 'array'::text))
);

CREATE VIEW public.singbox_external_node_states AS
 SELECT n.public_id AS id,
    n.source_id,
    n.identity_epoch,
    v.public_id AS current_version_id,
    COALESCE((m.public_preview ->> 'name'::text), (v.public_preview ->> 'name'::text), ''::text) AS name,
    n.adopted,
    COALESCE((n.last_seen_revision = ( SELECT r.id
           FROM public.singbox_subscription_source_revisions r
          WHERE ((r.source_id = n.source_id) AND (r.identity_epoch = n.identity_epoch))
          ORDER BY r.generation DESC
         LIMIT 1)), false) AS present,
    (n.identity_state = 'unique'::text) AS identity_unique,
    s.name AS source_name,
    s.identity_epoch AS source_epoch,
    s.archived AS source_archived,
    s.deleted_at AS source_deleted_at,
    (s.last_error ->> 'kind'::text) AS source_last_error,
    s.settings_revision AS source_settings_revision
   FROM (((public.singbox_ordered_external_nodes n
     JOIN public.singbox_ordered_subscription_sources s ON ((s.id = n.source_id)))
     LEFT JOIN public.singbox_ordered_external_node_versions v ON ((v.id = n.latest_version)))
     LEFT JOIN public.singbox_subscription_revision_nodes m ON (((m.node_id = n.id) AND (m.source_revision_id = n.last_seen_revision))));

CREATE VIEW public.singbox_external_version_configs AS
 SELECT v.public_id AS id,
    n.public_id AS external_node_id,
    n.source_id,
    n.identity_epoch,
    r.parser_version,
    true AS ordered,
    v.legacy_config AS config_json,
    v.legacy_config_sha256 AS config_sha256,
    v.normalized_config,
    v.content_digest
   FROM ((public.singbox_ordered_external_node_versions v
     JOIN public.singbox_ordered_external_nodes n ON ((n.id = v.node_id)))
     JOIN public.singbox_subscription_source_revisions r ON ((r.id = v.source_revision_id)));

CREATE TABLE public.singbox_installation (
    server_id bigint NOT NULL,
    target_rev bigint NOT NULL,
    error text,
    checked_at bigint NOT NULL,
    CONSTRAINT singbox_installation_error_check CHECK (((error IS NULL) OR ((char_length(error) >= 1) AND (char_length(error) <= 2048)))),
    CONSTRAINT singbox_installation_target_rev_check CHECK ((target_rev > 0))
);

CREATE TABLE public.singbox_invitation_policy_groups (
    group_id bigint NOT NULL
);

CREATE TABLE public.singbox_invitation_quota_events (
    id bigint NOT NULL,
    user_id bigint NOT NULL,
    delta integer NOT NULL,
    remaining integer NOT NULL,
    reason text NOT NULL,
    actor text NOT NULL,
    detail jsonb DEFAULT '{}'::jsonb NOT NULL,
    created_at bigint NOT NULL,
    CONSTRAINT singbox_invitation_quota_events_reason_check CHECK ((reason = ANY (ARRAY['balance_conversion'::text, 'administrator'::text, 'invitation_created'::text, 'invitation_revoked'::text, 'invited_account'::text])))
);

CREATE SEQUENCE public.singbox_invitation_quota_events_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

ALTER SEQUENCE public.singbox_invitation_quota_events_id_seq OWNED BY public.singbox_invitation_quota_events.id;

CREATE TABLE public.singbox_invitation_quotas (
    user_id bigint NOT NULL,
    remaining integer DEFAULT 0 NOT NULL,
    CONSTRAINT singbox_invitation_quotas_remaining_check CHECK (((remaining >= 0) AND (remaining <= 100000)))
);

CREATE TABLE public.singbox_invitation_settings (
    singleton boolean DEFAULT true NOT NULL,
    enabled boolean DEFAULT false NOT NULL,
    package_group_id bigint,
    default_quota integer DEFAULT 0 NOT NULL,
    version bigint DEFAULT 1 NOT NULL,
    updated_at bigint DEFAULT 0 NOT NULL,
    CONSTRAINT singbox_invitation_settings_default_quota_check CHECK (((default_quota >= 0) AND (default_quota <= 1000))),
    CONSTRAINT singbox_invitation_settings_singleton_check CHECK (singleton)
);

CREATE TABLE public.singbox_invitations (
    id uuid NOT NULL,
    issuer_id bigint NOT NULL,
    token_hash text NOT NULL,
    hint text DEFAULT ''::text NOT NULL,
    origin text DEFAULT 'quota'::text NOT NULL,
    request_id uuid,
    state text DEFAULT 'available'::text NOT NULL,
    recipient_id bigint,
    claim_hash text,
    created_at bigint NOT NULL,
    claimed_at bigint,
    revoked_at bigint,
    CONSTRAINT singbox_invitations_check CHECK (((state = 'claimed'::text) = (recipient_id IS NOT NULL))),
    CONSTRAINT singbox_invitations_origin_check CHECK ((origin = ANY (ARRAY['quota'::text, 'paid'::text]))),
    CONSTRAINT singbox_invitations_state_check CHECK ((state = ANY (ARRAY['available'::text, 'claimed'::text, 'revoked'::text])))
);

CREATE TABLE public.singbox_mail_settings (
    singleton boolean DEFAULT true NOT NULL,
    enabled boolean DEFAULT false NOT NULL,
    reminders_enabled boolean DEFAULT false NOT NULL,
    host text DEFAULT ''::text NOT NULL,
    port integer DEFAULT 465 NOT NULL,
    security text DEFAULT 'tls'::text NOT NULL,
    username text DEFAULT ''::text NOT NULL,
    from_address text DEFAULT ''::text NOT NULL,
    credential_id uuid,
    blocked_domains text[] DEFAULT '{}'::text[] NOT NULL,
    version bigint DEFAULT 1 NOT NULL,
    CONSTRAINT singbox_mail_settings_port_check CHECK (((port >= 1) AND (port <= 65535))),
    CONSTRAINT singbox_mail_settings_security_check CHECK ((security = ANY (ARRAY['tls'::text, 'starttls'::text]))),
    CONSTRAINT singbox_mail_settings_singleton_check CHECK (singleton)
);

CREATE TABLE public.singbox_managed_endpoint_versions (
    id uuid NOT NULL,
    node_id bigint NOT NULL,
    server_id bigint NOT NULL,
    snapshot jsonb NOT NULL,
    semantic_sha256 text NOT NULL,
    created_at bigint NOT NULL,
    CONSTRAINT singbox_managed_endpoint_versions_semantic_sha256_check CHECK ((semantic_sha256 ~ '^[0-9a-f]{64}$'::text)),
    CONSTRAINT singbox_managed_endpoint_versions_snapshot_check CHECK ((jsonb_typeof(snapshot) = 'object'::text))
);

CREATE TABLE public.singbox_member_content (
    id bigint NOT NULL,
    kind text NOT NULL,
    title text NOT NULL,
    category text DEFAULT ''::text NOT NULL,
    body text NOT NULL,
    published boolean DEFAULT false NOT NULL,
    version bigint DEFAULT 1 NOT NULL,
    updated_at bigint NOT NULL,
    CONSTRAINT singbox_member_content_kind_check CHECK ((kind = ANY (ARRAY['notice'::text, 'guide'::text])))
);

CREATE SEQUENCE public.singbox_member_content_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

ALTER SEQUENCE public.singbox_member_content_id_seq OWNED BY public.singbox_member_content.id;

CREATE TABLE public.singbox_member_messages (
    id bigint NOT NULL,
    user_id bigint NOT NULL,
    event_key text NOT NULL,
    kind text NOT NULL,
    message text NOT NULL,
    created_at bigint NOT NULL,
    read_at bigint,
    mail_state text DEFAULT 'disabled'::text NOT NULL,
    mail_attempts integer DEFAULT 0 NOT NULL,
    next_attempt_at bigint DEFAULT 0 NOT NULL,
    lease_until bigint,
    CONSTRAINT singbox_member_messages_mail_state_check CHECK ((mail_state = ANY (ARRAY['disabled'::text, 'pending'::text, 'sending'::text, 'sent'::text, 'failed'::text, 'cancelled'::text])))
);

CREATE SEQUENCE public.singbox_member_messages_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

ALTER SEQUENCE public.singbox_member_messages_id_seq OWNED BY public.singbox_member_messages.id;

CREATE TABLE public.singbox_members (
    user_id bigint NOT NULL,
    email text,
    state text DEFAULT 'active'::text NOT NULL,
    inviter_id bigint,
    sponsor_required boolean DEFAULT false NOT NULL,
    first_purchase_at bigint,
    version bigint DEFAULT 1 NOT NULL,
    notes text DEFAULT ''::text NOT NULL,
    created_at bigint NOT NULL,
    email_verified_at bigint,
    password_hash text,
    remind_expire boolean DEFAULT true NOT NULL,
    remind_traffic boolean DEFAULT true NOT NULL,
    remind_invitation boolean DEFAULT true NOT NULL,
    legacy_login_email text,
    CONSTRAINT singbox_members_check CHECK (((inviter_id IS NULL) OR (inviter_id <> user_id))),
    CONSTRAINT singbox_members_state_check CHECK ((state = ANY (ARRAY['active'::text, 'pending'::text, 'suspended'::text, 'archived'::text, 'staged'::text])))
);

CREATE TABLE public.singbox_nexus_bridge (
    singleton boolean DEFAULT true NOT NULL,
    import_id uuid NOT NULL,
    subscription_base text NOT NULL,
    enabled boolean DEFAULT false NOT NULL,
    pending_generation uuid,
    last_sync_at bigint,
    activated_at bigint NOT NULL,
    CONSTRAINT singbox_nexus_bridge_singleton_check CHECK (singleton)
);

CREATE TABLE public.singbox_nexus_device_state (
    user_id bigint NOT NULL,
    device_limit integer NOT NULL,
    legacy_addresses jsonb DEFAULT '[]'::jsonb NOT NULL,
    observed_at bigint NOT NULL,
    CONSTRAINT singbox_nexus_device_state_device_limit_check CHECK (((device_limit >= 0) AND (device_limit <= 1024))),
    CONSTRAINT singbox_nexus_device_state_legacy_addresses_check CHECK ((jsonb_typeof(legacy_addresses) = 'array'::text))
);

CREATE TABLE public.singbox_nexus_identities (
    user_id bigint NOT NULL,
    import_id uuid NOT NULL,
    legacy_uuid uuid NOT NULL,
    original_state text NOT NULL,
    original_plan_id bigint,
    quota_bytes numeric(30,0),
    used_upload numeric(30,0) NOT NULL,
    used_download numeric(30,0) NOT NULL,
    expires_at bigint,
    reset_at bigint
);

CREATE TABLE public.singbox_nexus_imports (
    id uuid NOT NULL,
    source_id text NOT NULL,
    digest text NOT NULL,
    counts jsonb NOT NULL,
    imported_at bigint NOT NULL,
    administrator_id bigint NOT NULL
);

CREATE TABLE public.singbox_nexus_metering (
    user_id bigint NOT NULL,
    upload numeric(30,0) NOT NULL,
    download numeric(30,0) NOT NULL,
    last_reset_at bigint,
    next_reset_at bigint,
    observed_at bigint NOT NULL,
    CONSTRAINT singbox_nexus_metering_download_check CHECK ((download >= (0)::numeric)),
    CONSTRAINT singbox_nexus_metering_upload_check CHECK ((upload >= (0)::numeric))
);

CREATE TABLE public.singbox_nexus_node_imports (
    source_id text NOT NULL,
    digest text NOT NULL,
    infrastructure_digest text NOT NULL,
    exported_at bigint NOT NULL,
    imported_at bigint NOT NULL,
    node_count integer NOT NULL,
    CONSTRAINT singbox_nexus_node_imports_digest_check CHECK ((length(digest) = 64)),
    CONSTRAINT singbox_nexus_node_imports_infrastructure_digest_check CHECK ((length(infrastructure_digest) = 64)),
    CONSTRAINT singbox_nexus_node_imports_node_count_check CHECK ((node_count > 0))
);

CREATE TABLE public.singbox_nexus_node_status (
    legacy_node_id bigint NOT NULL,
    online boolean NOT NULL,
    checked_at bigint,
    load jsonb DEFAULT '{}'::jsonb NOT NULL,
    latency jsonb DEFAULT '{}'::jsonb NOT NULL,
    observed_at bigint NOT NULL,
    CONSTRAINT singbox_nexus_node_status_latency_check CHECK ((jsonb_typeof(latency) = 'object'::text)),
    CONSTRAINT singbox_nexus_node_status_legacy_node_id_check CHECK ((legacy_node_id > 0)),
    CONSTRAINT singbox_nexus_node_status_load_check CHECK ((jsonb_typeof(load) = 'object'::text))
);

CREATE TABLE public.singbox_nexus_nodes (
    id bigint NOT NULL,
    source_id text NOT NULL,
    legacy_node_id bigint NOT NULL,
    legacy_machine_id bigint NOT NULL,
    legacy_host_id bigint NOT NULL,
    server_id bigint NOT NULL,
    credential_id uuid NOT NULL,
    display jsonb NOT NULL,
    CONSTRAINT singbox_nexus_nodes_display_check CHECK ((jsonb_typeof(display) = 'object'::text)),
    CONSTRAINT singbox_nexus_nodes_legacy_host_id_check CHECK ((legacy_host_id > 0)),
    CONSTRAINT singbox_nexus_nodes_legacy_machine_id_check CHECK ((legacy_machine_id > 0)),
    CONSTRAINT singbox_nexus_nodes_legacy_node_id_check CHECK ((legacy_node_id > 0))
);

CREATE SEQUENCE public.singbox_nexus_nodes_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

ALTER SEQUENCE public.singbox_nexus_nodes_id_seq OWNED BY public.singbox_nexus_nodes.id;

CREATE TABLE public.singbox_nexus_plan_mappings (
    import_id uuid NOT NULL,
    package_group_id bigint NOT NULL,
    legacy_plan_id bigint NOT NULL,
    legacy_group_id bigint
);

CREATE TABLE public.singbox_nexus_runtime_hosts (
    server_id bigint NOT NULL,
    source_id text NOT NULL,
    credential_id uuid NOT NULL,
    digest text NOT NULL,
    state text DEFAULT 'prepared'::text NOT NULL,
    membership_digest text,
    created_at bigint NOT NULL,
    activated_at bigint,
    evidence jsonb DEFAULT '{}'::jsonb NOT NULL,
    session_secret text DEFAULT (replace((gen_random_uuid())::text, '-'::text, ''::text) || replace((gen_random_uuid())::text, '-'::text, ''::text)) NOT NULL,
    last_activated_at bigint,
    CONSTRAINT singbox_nexus_runtime_hosts_digest_check CHECK ((length(digest) = 64)),
    CONSTRAINT singbox_nexus_runtime_hosts_session_secret_check CHECK ((session_secret ~ '^[0-9a-f]{64}$'::text)),
    CONSTRAINT singbox_nexus_runtime_hosts_state_check CHECK ((state = ANY (ARRAY['prepared'::text, 'active'::text, 'paused'::text])))
);

CREATE TABLE public.singbox_nexus_usage_deliveries (
    id bigint NOT NULL,
    server_id bigint NOT NULL,
    epoch uuid NOT NULL,
    seq numeric(20,0) NOT NULL,
    stat_name text NOT NULL,
    user_id bigint NOT NULL,
    uplink numeric(30,0) NOT NULL,
    downlink numeric(30,0) NOT NULL,
    rate numeric(12,6) NOT NULL,
    period_start bigint NOT NULL,
    period_end bigint NOT NULL,
    delivered_at bigint
);

CREATE SEQUENCE public.singbox_nexus_usage_deliveries_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

ALTER SEQUENCE public.singbox_nexus_usage_deliveries_id_seq OWNED BY public.singbox_nexus_usage_deliveries.id;

CREATE TABLE public.singbox_node_metadata (
    kind text NOT NULL,
    id bigint NOT NULL,
    node_id bigint GENERATED ALWAYS AS (
CASE
    WHEN (kind = 'direct'::text) THEN id
    ELSE NULL::bigint
END) STORED,
    chain_id bigint GENERATED ALWAYS AS (
CASE
    WHEN (kind = 'chain'::text) THEN id
    ELSE NULL::bigint
END) STORED,
    external_node_id bigint GENERATED ALWAYS AS (
CASE
    WHEN (kind = 'external'::text) THEN id
    ELSE NULL::bigint
END) STORED,
    name_override text,
    tags jsonb DEFAULT '[]'::jsonb NOT NULL,
    note text DEFAULT ''::text NOT NULL,
    sort_order bigint DEFAULT 0 NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    revision bigint DEFAULT 1 NOT NULL,
    deleted_at bigint,
    CONSTRAINT singbox_node_metadata_check CHECK (((kind = 'external'::text) OR (name_override IS NULL))),
    CONSTRAINT singbox_node_metadata_id_check CHECK ((id > 0)),
    CONSTRAINT singbox_node_metadata_kind_check CHECK ((kind = ANY (ARRAY['direct'::text, 'chain'::text, 'external'::text]))),
    CONSTRAINT singbox_node_metadata_name_override_check CHECK (((name_override IS NULL) OR ((char_length(name_override) >= 1) AND (char_length(name_override) <= 128)))),
    CONSTRAINT singbox_node_metadata_note_check CHECK ((octet_length(note) <= 1024)),
    CONSTRAINT singbox_node_metadata_revision_check CHECK ((revision > 0)),
    CONSTRAINT singbox_node_metadata_tags_check CHECK (((jsonb_typeof(tags) = 'array'::text) AND (jsonb_array_length(tags) <= 16)))
);

CREATE TABLE public.singbox_operation_events (
    id bigint NOT NULL,
    administrator_id bigint,
    user_id bigint,
    action text NOT NULL,
    detail jsonb NOT NULL,
    created_at bigint NOT NULL
);

CREATE SEQUENCE public.singbox_operation_events_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

ALTER SEQUENCE public.singbox_operation_events_id_seq OWNED BY public.singbox_operation_events.id;

CREATE TABLE public.singbox_operation_previews (
    id uuid NOT NULL,
    administrator_id bigint NOT NULL,
    request jsonb NOT NULL,
    fingerprint text NOT NULL,
    summary jsonb NOT NULL,
    created_at bigint NOT NULL,
    expires_at bigint NOT NULL,
    receipt jsonb
);

CREATE TABLE public.singbox_operation_privacy (
    singleton boolean DEFAULT true NOT NULL,
    administrator_days integer DEFAULT 180 NOT NULL,
    subscription_days integer DEFAULT 7 NOT NULL,
    security_days integer DEFAULT 90 NOT NULL,
    CONSTRAINT singbox_operation_privacy_administrator_days_check CHECK (((administrator_days >= 1) AND (administrator_days <= 3650))),
    CONSTRAINT singbox_operation_privacy_security_days_check CHECK (((security_days >= 1) AND (security_days <= 3650))),
    CONSTRAINT singbox_operation_privacy_singleton_check CHECK (singleton),
    CONSTRAINT singbox_operation_privacy_subscription_days_check CHECK (((subscription_days >= 0) AND (subscription_days <= 365)))
);

CREATE TABLE public.singbox_ordered_chain_creation_requests (
    request_id uuid NOT NULL,
    request_sha256 text NOT NULL,
    receipt jsonb NOT NULL,
    created_at bigint NOT NULL,
    CONSTRAINT singbox_ordered_chain_creation_requests_created_at_check CHECK ((created_at >= 0)),
    CONSTRAINT singbox_ordered_chain_creation_requests_receipt_check CHECK ((jsonb_typeof(receipt) = 'object'::text)),
    CONSTRAINT singbox_ordered_chain_creation_requests_request_id_check CHECK ((request_id <> '00000000-0000-0000-0000-000000000000'::uuid)),
    CONSTRAINT singbox_ordered_chain_creation_requests_request_sha256_check CHECK ((request_sha256 ~ '^[0-9a-f]{64}$'::text))
);

CREATE TABLE public.singbox_ordered_source_previews (
    id uuid NOT NULL,
    admin_id bigint NOT NULL,
    input_config jsonb NOT NULL,
    body bytea NOT NULL,
    user_agent text,
    parser_version text NOT NULL,
    traffic jsonb DEFAULT '{}'::jsonb NOT NULL,
    created_at bigint NOT NULL,
    expires_at bigint NOT NULL,
    CONSTRAINT singbox_ordered_source_previews_body_check CHECK (((octet_length(body) >= 1) AND (octet_length(body) <= 2097152))),
    CONSTRAINT singbox_ordered_source_previews_check CHECK (((expires_at > created_at) AND (expires_at <= (created_at + 600)))),
    CONSTRAINT singbox_ordered_source_previews_id_check CHECK ((id <> '00000000-0000-0000-0000-000000000000'::uuid)),
    CONSTRAINT singbox_ordered_source_previews_input_config_check CHECK ((jsonb_typeof(input_config) = 'object'::text)),
    CONSTRAINT singbox_ordered_source_previews_traffic_check CHECK ((jsonb_typeof(traffic) = 'object'::text)),
    CONSTRAINT singbox_ordered_source_previews_user_agent_check CHECK (((user_agent IS NULL) OR ((octet_length(user_agent) >= 1) AND (octet_length(user_agent) <= 256))))
);

CREATE SEQUENCE public.singbox_ordered_subscription_sources_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

ALTER SEQUENCE public.singbox_ordered_subscription_sources_id_seq OWNED BY public.singbox_ordered_subscription_sources.id;

CREATE TABLE public.singbox_package_assignments (
    id bigint NOT NULL,
    user_id bigint NOT NULL,
    request_id uuid NOT NULL,
    package_group_id bigint NOT NULL,
    package_name text NOT NULL,
    monthly_bytes numeric(20,0),
    reset_day integer NOT NULL,
    reset_hour integer NOT NULL,
    reset_minute integer NOT NULL,
    timezone text NOT NULL,
    starts_at bigint NOT NULL,
    expires_at bigint NOT NULL,
    CONSTRAINT singbox_package_assignments_check CHECK ((expires_at > starts_at)),
    CONSTRAINT singbox_package_assignments_monthly_bytes_check CHECK (((monthly_bytes > (0)::numeric) AND (monthly_bytes <= '18446744073709551615'::numeric))),
    CONSTRAINT singbox_package_assignments_reset_day_check CHECK (((reset_day >= 1) AND (reset_day <= 31))),
    CONSTRAINT singbox_package_assignments_reset_hour_check CHECK (((reset_hour >= 0) AND (reset_hour <= 23))),
    CONSTRAINT singbox_package_assignments_reset_minute_check CHECK (((reset_minute >= 0) AND (reset_minute <= 59)))
);

CREATE SEQUENCE public.singbox_package_assignments_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

ALTER SEQUENCE public.singbox_package_assignments_id_seq OWNED BY public.singbox_package_assignments.id;

CREATE TABLE public.singbox_package_groups (
    id bigint NOT NULL,
    name text NOT NULL,
    monthly_bytes numeric(20,0),
    reset_day integer NOT NULL,
    reset_hour integer NOT NULL,
    reset_minute integer NOT NULL,
    timezone text NOT NULL,
    duration_days integer NOT NULL,
    deleted_at bigint,
    CONSTRAINT singbox_package_groups_duration_days_check CHECK (((duration_days >= 1) AND (duration_days <= 36500))),
    CONSTRAINT singbox_package_groups_monthly_bytes_check CHECK (((monthly_bytes > (0)::numeric) AND (monthly_bytes <= '18446744073709551615'::numeric))),
    CONSTRAINT singbox_package_groups_reset_day_check CHECK (((reset_day >= 1) AND (reset_day <= 31))),
    CONSTRAINT singbox_package_groups_reset_hour_check CHECK (((reset_hour >= 0) AND (reset_hour <= 23))),
    CONSTRAINT singbox_package_groups_reset_minute_check CHECK (((reset_minute >= 0) AND (reset_minute <= 59)))
);

CREATE SEQUENCE public.singbox_package_groups_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

ALTER SEQUENCE public.singbox_package_groups_id_seq OWNED BY public.singbox_package_groups.id;

CREATE TABLE public.singbox_path_controller_secrets (
    server_id bigint NOT NULL,
    secret text NOT NULL,
    CONSTRAINT singbox_path_controller_secrets_secret_check CHECK ((secret ~ '^[0-9a-f]{64}$'::text))
);

CREATE TABLE public.singbox_path_deployment_dependencies (
    server_id bigint NOT NULL,
    module text DEFAULT 'singbox'::text NOT NULL,
    revision bigint NOT NULL,
    chain_id bigint NOT NULL,
    generation bigint NOT NULL,
    role text NOT NULL,
    hop_position integer NOT NULL,
    route_active boolean NOT NULL,
    CONSTRAINT singbox_path_deployment_dependencies_hop_position_check CHECK (((hop_position >= 0) AND (hop_position <= 8))),
    CONSTRAINT singbox_path_deployment_dependencies_role_check CHECK ((role = ANY (ARRAY['entry'::text, 'managed'::text])))
);

CREATE TABLE public.singbox_path_probes (
    probe_id uuid NOT NULL,
    chain_id bigint NOT NULL,
    generation bigint NOT NULL,
    stage text NOT NULL,
    request_id uuid,
    server_id bigint NOT NULL,
    revision bigint,
    state text DEFAULT 'pending'::text NOT NULL,
    observed_at bigint,
    error text,
    dependency_vector jsonb,
    created_at bigint NOT NULL,
    CONSTRAINT singbox_path_probes_stage_check CHECK ((stage = ANY (ARRAY['candidate'::text, 'switched'::text]))),
    CONSTRAINT singbox_path_probes_state_check CHECK ((state = ANY (ARRAY['pending'::text, 'verified'::text, 'failed'::text, 'expired'::text])))
);

CREATE TABLE public.singbox_path_stage_deployments (
    chain_id bigint NOT NULL,
    generation bigint NOT NULL,
    stage text NOT NULL,
    server_id bigint NOT NULL,
    role text NOT NULL,
    hop_position integer NOT NULL,
    revision bigint NOT NULL,
    bundle_sha256 text NOT NULL,
    deployment_id uuid NOT NULL,
    binding_digest text NOT NULL,
    checkpoint_request_id uuid,
    barrier_request_id uuid,
    barrier_vector jsonb,
    observed_at bigint,
    module text DEFAULT 'singbox'::text NOT NULL,
    CONSTRAINT singbox_path_stage_deployments_binding_digest_check CHECK ((binding_digest ~ '^[0-9a-f]{64}$'::text)),
    CONSTRAINT singbox_path_stage_deployments_bundle_sha256_check CHECK ((bundle_sha256 ~ '^[0-9a-f]{64}$'::text)),
    CONSTRAINT singbox_path_stage_deployments_hop_position_check CHECK (((hop_position >= 0) AND (hop_position <= 8))),
    CONSTRAINT singbox_path_stage_deployments_role_check CHECK ((role = ANY (ARRAY['entry'::text, 'managed'::text])))
);

CREATE TABLE public.singbox_payment_channels (
    id bigint NOT NULL,
    name text NOT NULL,
    gateway text NOT NULL,
    merchant text NOT NULL,
    credential_id uuid NOT NULL,
    payment_type text NOT NULL,
    fixed_fee_cents bigint DEFAULT 0 NOT NULL,
    fee_basis_points integer DEFAULT 0 NOT NULL,
    enabled boolean DEFAULT false NOT NULL,
    version bigint DEFAULT 1 NOT NULL,
    updated_at bigint NOT NULL,
    CONSTRAINT singbox_payment_channels_fee_basis_points_check CHECK (((fee_basis_points >= 0) AND (fee_basis_points <= 10000))),
    CONSTRAINT singbox_payment_channels_fixed_fee_cents_check CHECK (((fixed_fee_cents >= 0) AND (fixed_fee_cents <= 1000000))),
    CONSTRAINT singbox_payment_channels_payment_type_check CHECK ((payment_type = ANY (ARRAY['alipay'::text, 'wxpay'::text, 'qqpay'::text])))
);

CREATE SEQUENCE public.singbox_payment_channels_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

ALTER SEQUENCE public.singbox_payment_channels_id_seq OWNED BY public.singbox_payment_channels.id;

CREATE TABLE public.singbox_policy_external_nodes (
    group_id bigint NOT NULL,
    external_node_id bigint NOT NULL,
    identity_epoch bigint NOT NULL,
    node_version_id bigint NOT NULL,
    update_mode text NOT NULL,
    created_at bigint NOT NULL,
    CONSTRAINT singbox_policy_external_nodes_identity_epoch_check CHECK ((identity_epoch > 0)),
    CONSTRAINT singbox_policy_external_nodes_update_mode_check CHECK ((update_mode = ANY (ARRAY['follow_node'::text, 'pinned'::text])))
);

CREATE TABLE public.singbox_policy_groups (
    id bigint NOT NULL,
    name text NOT NULL,
    legacy_group_id bigint,
    CONSTRAINT singbox_policy_groups_legacy_group_id_check CHECK ((legacy_group_id > 0))
);

CREATE SEQUENCE public.singbox_policy_groups_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

ALTER SEQUENCE public.singbox_policy_groups_id_seq OWNED BY public.singbox_policy_groups.id;

CREATE TABLE public.singbox_portal_accounts (
    user_id bigint NOT NULL,
    account_id uuid NOT NULL,
    activation_hash text,
    activation_expires_at bigint,
    CONSTRAINT singbox_portal_accounts_check CHECK (((activation_hash IS NULL) = (activation_expires_at IS NULL)))
);

CREATE TABLE public.singbox_portal_sessions (
    token_hash text NOT NULL,
    account_id uuid NOT NULL,
    expires_at bigint NOT NULL,
    verified_at bigint NOT NULL,
    proof text DEFAULT 'password'::text NOT NULL,
    CONSTRAINT singbox_portal_sessions_proof_check CHECK ((proof = ANY (ARRAY['passkey'::text, 'password'::text])))
);

CREATE TABLE public.singbox_quota_credits (
    id uuid NOT NULL,
    user_id bigint NOT NULL,
    cycle_start bigint NOT NULL,
    next_reset bigint NOT NULL,
    bytes numeric(30,0) NOT NULL,
    created_at bigint NOT NULL,
    administrator_id bigint NOT NULL,
    CONSTRAINT singbox_quota_credits_bytes_check CHECK ((bytes >= (0)::numeric)),
    CONSTRAINT singbox_quota_credits_check CHECK ((next_reset > cycle_start))
);

CREATE TABLE public.singbox_reachability_deliveries (
    id bigint NOT NULL,
    event_id uuid NOT NULL,
    channel text NOT NULL,
    payload text NOT NULL,
    state text DEFAULT 'pending'::text NOT NULL,
    attempts integer DEFAULT 0 NOT NULL,
    next_attempt_at bigint NOT NULL,
    lease_until bigint,
    last_error text,
    CONSTRAINT singbox_reachability_deliveries_channel_check CHECK ((channel = ANY (ARRAY['telegram'::text, 'webhook'::text]))),
    CONSTRAINT singbox_reachability_deliveries_state_check CHECK ((state = ANY (ARRAY['pending'::text, 'sending'::text, 'sent'::text, 'failed'::text, 'cancelled'::text])))
);

CREATE SEQUENCE public.singbox_reachability_deliveries_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

ALTER SEQUENCE public.singbox_reachability_deliveries_id_seq OWNED BY public.singbox_reachability_deliveries.id;

CREATE TABLE public.singbox_reachability_events (
    id uuid NOT NULL,
    target_id bigint,
    server_id bigint,
    kind text NOT NULL,
    from_state text NOT NULL,
    to_state text NOT NULL,
    message text NOT NULL,
    created_at bigint NOT NULL,
    CONSTRAINT singbox_reachability_events_kind_check CHECK ((kind = ANY (ARRAY['blocked'::text, 'cleared'::text])))
);

CREATE TABLE public.singbox_reachability_history (
    id bigint NOT NULL,
    target_id bigint NOT NULL,
    checked_at bigint NOT NULL,
    state text NOT NULL,
    observed_state text NOT NULL,
    detail jsonb NOT NULL
);

CREATE SEQUENCE public.singbox_reachability_history_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

ALTER SEQUENCE public.singbox_reachability_history_id_seq OWNED BY public.singbox_reachability_history.id;

CREATE TABLE public.singbox_reachability_settings (
    singleton boolean DEFAULT true NOT NULL,
    enabled boolean DEFAULT false NOT NULL,
    interval_minutes integer DEFAULT 30 NOT NULL,
    credential_id uuid,
    revision bigint DEFAULT 1 NOT NULL,
    next_run_at bigint DEFAULT 0 NOT NULL,
    run_requested_at bigint,
    lease_until bigint,
    last_run jsonb DEFAULT '{}'::jsonb NOT NULL,
    CONSTRAINT singbox_reachability_settings_interval_minutes_check CHECK (((interval_minutes >= 15) AND (interval_minutes <= 1440))),
    CONSTRAINT singbox_reachability_settings_last_run_check CHECK ((jsonb_typeof(last_run) = 'object'::text)),
    CONSTRAINT singbox_reachability_settings_revision_check CHECK ((revision > 0)),
    CONSTRAINT singbox_reachability_settings_singleton_check CHECK (singleton)
);

CREATE TABLE public.singbox_reachability_targets (
    id bigint NOT NULL,
    host text NOT NULL,
    port integer NOT NULL,
    state text DEFAULT 'unknown'::text NOT NULL,
    pending_state text,
    failing_streak integer DEFAULT 0 NOT NULL,
    observed_state text,
    resolved_ip text,
    detail jsonb DEFAULT '{}'::jsonb NOT NULL,
    checked_at bigint,
    state_changed_at bigint,
    deferred_at bigint,
    seen_at bigint NOT NULL,
    CONSTRAINT singbox_reachability_targets_detail_check CHECK ((jsonb_typeof(detail) = 'object'::text)),
    CONSTRAINT singbox_reachability_targets_failing_streak_check CHECK ((failing_streak >= 0)),
    CONSTRAINT singbox_reachability_targets_host_check CHECK (((host <> ''::text) AND (length(host) <= 253))),
    CONSTRAINT singbox_reachability_targets_observed_state_check CHECK ((observed_state = ANY (ARRAY['reachable'::text, 'partial'::text, 'service_down'::text, 'port_blocked'::text, 'ip_blocked'::text, 'unconfirmed'::text, 'unknown'::text, 'not_applicable'::text]))),
    CONSTRAINT singbox_reachability_targets_pending_state_check CHECK ((pending_state = ANY (ARRAY['port_blocked'::text, 'ip_blocked'::text, 'unconfirmed'::text]))),
    CONSTRAINT singbox_reachability_targets_port_check CHECK (((port >= 1) AND (port <= 65535))),
    CONSTRAINT singbox_reachability_targets_state_check CHECK ((state = ANY (ARRAY['reachable'::text, 'partial'::text, 'service_down'::text, 'port_blocked'::text, 'ip_blocked'::text, 'unconfirmed'::text, 'unknown'::text, 'not_applicable'::text])))
);

CREATE SEQUENCE public.singbox_reachability_targets_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

ALTER SEQUENCE public.singbox_reachability_targets_id_seq OWNED BY public.singbox_reachability_targets.id;

CREATE TABLE public.singbox_resource_mutation_requests (
    request_id uuid NOT NULL,
    request_sha256 text NOT NULL,
    receipt jsonb NOT NULL,
    created_at bigint NOT NULL,
    CONSTRAINT singbox_resource_mutation_requests_request_sha256_check CHECK ((request_sha256 ~ '^[0-9a-f]{64}$'::text))
);

CREATE TABLE public.singbox_runtime_manifest_facts (
    server_id bigint NOT NULL,
    module text DEFAULT 'singbox'::text NOT NULL,
    revision bigint NOT NULL,
    runtime_version text NOT NULL,
    artifact_sha256 text NOT NULL,
    artifact jsonb NOT NULL,
    CONSTRAINT singbox_runtime_manifest_facts_runtime_version_check CHECK ((runtime_version = ANY (ARRAY['1.14.2'::text, '1.14.2-apsp.1'::text, '1.14.0-apsp.1'::text, '1.14.4-apsp.1'::text, '1.14.4-apsp.2'::text])))
);

CREATE TABLE public.singbox_runtime_rollout_members (
    rollout_id uuid NOT NULL,
    server_id bigint NOT NULL,
    baseline_revision bigint NOT NULL,
    artifact_sha256 text NOT NULL,
    inspect_request_id uuid NOT NULL,
    inspection_result jsonb,
    dispatched_at bigint NOT NULL
);

CREATE TABLE public.singbox_runtime_rollouts (
    id uuid NOT NULL,
    administrator_id bigint NOT NULL,
    runtime_version text NOT NULL,
    targets bigint[] NOT NULL,
    batch_size integer NOT NULL,
    dispatched_count integer DEFAULT 0 NOT NULL,
    paused boolean DEFAULT false NOT NULL,
    artifact_snapshot jsonb NOT NULL,
    completed_at bigint,
    created_at bigint NOT NULL,
    updated_at bigint NOT NULL,
    CONSTRAINT singbox_runtime_rollouts_batch_size_check CHECK (((batch_size >= 1) AND (batch_size <= 20))),
    CONSTRAINT singbox_runtime_rollouts_dispatched_count_check CHECK ((dispatched_count >= 0)),
    CONSTRAINT singbox_runtime_rollouts_runtime_version_check CHECK ((runtime_version = ANY (ARRAY['1.14.2'::text, '1.14.2-apsp.1'::text, '1.14.0-apsp.1'::text, '1.14.4-apsp.1'::text, '1.14.4-apsp.2'::text])))
);

CREATE TABLE public.singbox_runtime_session_addresses (
    server_id bigint NOT NULL,
    stat_name text NOT NULL,
    user_id bigint NOT NULL,
    node_id bigint NOT NULL,
    address text NOT NULL,
    observed_at bigint NOT NULL
);

CREATE TABLE public.singbox_subscription_domain (
    singleton boolean DEFAULT true NOT NULL,
    enabled boolean NOT NULL,
    credential_id uuid NOT NULL,
    zone_id text NOT NULL,
    zone_name text NOT NULL,
    host_prefix text NOT NULL,
    target text NOT NULL,
    legacy_paths boolean NOT NULL,
    revision bigint DEFAULT 1 NOT NULL,
    updated_at bigint NOT NULL,
    CONSTRAINT singbox_subscription_domain_singleton_check CHECK (singleton)
);

CREATE TABLE public.singbox_subscription_host_retired (
    record_id text NOT NULL,
    zone_id text NOT NULL,
    retired_at bigint NOT NULL
);

CREATE TABLE public.singbox_subscription_hosts (
    user_id bigint NOT NULL,
    slug text NOT NULL,
    path_token text NOT NULL,
    record_id text,
    state text DEFAULT 'pending'::text NOT NULL,
    last_error text,
    updated_at bigint NOT NULL,
    CONSTRAINT singbox_subscription_hosts_state_check CHECK ((state = ANY (ARRAY['pending'::text, 'ready'::text, 'failed'::text])))
);

CREATE TABLE public.singbox_subscription_source_jobs (
    id uuid NOT NULL,
    source_id bigint NOT NULL,
    settings_revision bigint NOT NULL,
    identity_epoch bigint NOT NULL,
    parser_version text NOT NULL,
    status text NOT NULL,
    stage text NOT NULL,
    claim_token uuid,
    deadline_at bigint,
    created_at bigint NOT NULL,
    started_at bigint,
    finished_at bigint,
    source_revision_id uuid,
    error jsonb,
    CONSTRAINT singbox_subscription_source_jobs_check CHECK (((status = ANY (ARRAY['running'::text, 'cancelling'::text])) = (claim_token IS NOT NULL))),
    CONSTRAINT singbox_subscription_source_jobs_check1 CHECK (((status <> ALL (ARRAY['running'::text, 'cancelling'::text])) OR (deadline_at IS NOT NULL))),
    CONSTRAINT singbox_subscription_source_jobs_id_check CHECK ((id <> '00000000-0000-0000-0000-000000000000'::uuid)),
    CONSTRAINT singbox_subscription_source_jobs_identity_epoch_check CHECK ((identity_epoch > 0)),
    CONSTRAINT singbox_subscription_source_jobs_settings_revision_check CHECK ((settings_revision > 0)),
    CONSTRAINT singbox_subscription_source_jobs_stage_check CHECK ((stage = ANY (ARRAY['queued'::text, 'fetch'::text, 'parse'::text, 'store'::text, 'done'::text]))),
    CONSTRAINT singbox_subscription_source_jobs_status_check CHECK ((status = ANY (ARRAY['queued'::text, 'running'::text, 'cancelling'::text, 'succeeded'::text, 'unchanged'::text, 'failed'::text, 'cancelled'::text, 'superseded'::text])))
);

CREATE TABLE public.singbox_subscription_source_requests (
    request_id uuid NOT NULL,
    operation text NOT NULL,
    source_id bigint,
    request_sha256 text NOT NULL,
    receipt jsonb NOT NULL,
    created_at bigint NOT NULL,
    CONSTRAINT singbox_subscription_source_requests_receipt_check CHECK ((jsonb_typeof(receipt) = 'object'::text)),
    CONSTRAINT singbox_subscription_source_requests_request_id_check CHECK ((request_id <> '00000000-0000-0000-0000-000000000000'::uuid)),
    CONSTRAINT singbox_subscription_source_requests_request_sha256_check CHECK ((request_sha256 ~ '^[0-9a-f]{64}$'::text))
);

CREATE SEQUENCE public.singbox_subscription_source_revisions_generation_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

ALTER SEQUENCE public.singbox_subscription_source_revisions_generation_seq OWNED BY public.singbox_subscription_source_revisions.generation;

CREATE TABLE public.singbox_usage_daily (
    user_id bigint NOT NULL,
    day bigint NOT NULL,
    node_id bigint NOT NULL,
    uplink numeric NOT NULL,
    downlink numeric NOT NULL,
    records bigint NOT NULL,
    last_period_end bigint NOT NULL,
    CONSTRAINT singbox_usage_daily_day_check CHECK (((day % (86400)::bigint) = 0)),
    CONSTRAINT singbox_usage_daily_downlink_check CHECK ((downlink >= (0)::numeric)),
    CONSTRAINT singbox_usage_daily_records_check CHECK ((records > 0)),
    CONSTRAINT singbox_usage_daily_uplink_check CHECK ((uplink >= (0)::numeric))
);

CREATE TABLE public.singbox_user_packages (
    user_id bigint NOT NULL,
    assignment_id bigint NOT NULL
);

CREATE TABLE public.singbox_wallets (
    user_id bigint NOT NULL,
    balance_cents bigint DEFAULT 0 NOT NULL,
    CONSTRAINT singbox_wallets_balance_cents_check CHECK (((balance_cents >= 0) AND (balance_cents <= 2147483647)))
);

CREATE TABLE public.system_observer_reports (
    id bigint NOT NULL,
    observer_name text NOT NULL,
    target_origin text NOT NULL,
    observed_at bigint NOT NULL,
    received_at bigint NOT NULL,
    available boolean,
    elapsed_ms bigint,
    evidence jsonb NOT NULL
);

CREATE SEQUENCE public.system_observer_reports_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

ALTER SEQUENCE public.system_observer_reports_id_seq OWNED BY public.system_observer_reports.id;

CREATE TABLE public.system_worker_heartbeats (
    name text NOT NULL,
    status text NOT NULL,
    observed_at bigint NOT NULL,
    last_success_at bigint,
    details jsonb DEFAULT '{}'::jsonb NOT NULL,
    CONSTRAINT system_worker_heartbeats_status_check CHECK ((status = ANY (ARRAY['running'::text, 'healthy'::text, 'failed'::text])))
);

CREATE TABLE public.tcpquality_target_regions (
    probe_id uuid NOT NULL,
    region text NOT NULL,
    CONSTRAINT tcpquality_target_regions_region_check CHECK ((region = ANY (ARRAY['east_asia'::text, 'southeast_asia'::text, 'europe'::text, 'americas'::text, 'other'::text])))
);

CREATE TABLE public.telemetry_history (
    server_id bigint NOT NULL,
    resolution_secs integer NOT NULL,
    bucket_at bigint NOT NULL,
    summary jsonb NOT NULL,
    CONSTRAINT telemetry_history_resolution_secs_check CHECK ((resolution_secs = ANY (ARRAY[60, 300, 3600])))
);

CREATE TABLE public.telemetry_history_initialized (
    server_id bigint NOT NULL
);

CREATE TABLE public.telemetry_policy (
    singleton boolean DEFAULT true NOT NULL,
    history_retention_days integer DEFAULT 30 NOT NULL,
    CONSTRAINT telemetry_policy_history_retention_days_check CHECK (((history_retention_days >= 1) AND (history_retention_days <= 3650))),
    CONSTRAINT telemetry_policy_singleton_check CHECK (singleton)
);

CREATE TABLE public.telemetry_receipts (
    server_id bigint NOT NULL,
    id uuid NOT NULL,
    sampled_at bigint NOT NULL,
    digest text NOT NULL
);

CREATE TABLE public.telemetry_samples (
    server_id bigint NOT NULL,
    id uuid NOT NULL,
    sampled_at bigint NOT NULL,
    digest text NOT NULL,
    metrics jsonb NOT NULL
);

CREATE TABLE public.tool_advisory_observations (
    ecosystem text NOT NULL,
    name text NOT NULL,
    version text NOT NULL,
    status text NOT NULL,
    checked_at bigint NOT NULL,
    observed_at bigint,
    evidence jsonb DEFAULT '{}'::jsonb NOT NULL,
    error text,
    CONSTRAINT tool_advisory_observations_status_check CHECK ((status = ANY (ARRAY['lookup-error'::text, 'no-recorded-advisory'::text, 'advisories-incomplete'::text, 'advisories-recorded'::text])))
);

CREATE TABLE public.usage_batches (
    server_id bigint NOT NULL,
    epoch uuid NOT NULL,
    seq numeric(20,0) NOT NULL,
    payload_hash text NOT NULL,
    received_at bigint,
    last_replayed_at bigint,
    replay_count bigint DEFAULT 0 NOT NULL,
    CONSTRAINT usage_batches_replay_count_check CHECK ((replay_count >= 0)),
    CONSTRAINT usage_batches_seq_check CHECK ((seq >= (0)::numeric)),
    CONSTRAINT usage_batches_seq_check1 CHECK ((seq <= '18446744073709551615'::numeric))
);

CREATE TABLE public.usage_records (
    server_id bigint NOT NULL,
    epoch uuid NOT NULL,
    seq numeric(20,0) NOT NULL,
    stat_name text NOT NULL,
    user_id bigint NOT NULL,
    node_id bigint NOT NULL,
    uplink numeric(20,0) NOT NULL,
    downlink numeric(20,0) NOT NULL,
    period_start bigint NOT NULL,
    period_end bigint NOT NULL,
    CONSTRAINT usage_records_check CHECK ((period_end >= period_start)),
    CONSTRAINT usage_records_downlink_check CHECK ((downlink >= (0)::numeric)),
    CONSTRAINT usage_records_downlink_check1 CHECK ((downlink <= '18446744073709551615'::numeric)),
    CONSTRAINT usage_records_seq_check CHECK ((seq >= (0)::numeric)),
    CONSTRAINT usage_records_seq_check1 CHECK ((seq <= '18446744073709551615'::numeric)),
    CONSTRAINT usage_records_uplink_check CHECK ((uplink >= (0)::numeric)),
    CONSTRAINT usage_records_uplink_check1 CHECK ((uplink <= '18446744073709551615'::numeric))
);

CREATE SEQUENCE public.users_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

ALTER SEQUENCE public.users_id_seq OWNED BY public.users.id;

ALTER TABLE ONLY public.alicloud_deliveries ALTER COLUMN id SET DEFAULT nextval('public.alicloud_deliveries_id_seq'::regclass);

ALTER TABLE ONLY public.alicloud_events ALTER COLUMN id SET DEFAULT nextval('public.alicloud_events_id_seq'::regclass);

ALTER TABLE ONLY public.diagnostic_jobs ALTER COLUMN generation SET DEFAULT nextval('public.diagnostic_jobs_generation_seq'::regclass);

ALTER TABLE ONLY public.expense_deliveries ALTER COLUMN id SET DEFAULT nextval('public.expense_deliveries_id_seq'::regclass);

ALTER TABLE ONLY public.expense_labels ALTER COLUMN id SET DEFAULT nextval('public.expense_labels_id_seq'::regclass);

ALTER TABLE ONLY public.expense_payments ALTER COLUMN id SET DEFAULT nextval('public.expense_payments_id_seq'::regclass);

ALTER TABLE ONLY public.expense_subscriptions ALTER COLUMN id SET DEFAULT nextval('public.expense_subscriptions_id_seq'::regclass);

ALTER TABLE ONLY public.management_audit ALTER COLUMN id SET DEFAULT nextval('public.management_audit_id_seq'::regclass);

ALTER TABLE ONLY public.nodes ALTER COLUMN id SET DEFAULT nextval('public.nodes_id_seq'::regclass);

ALTER TABLE ONLY public.notification_outbox ALTER COLUMN id SET DEFAULT nextval('public.notification_outbox_id_seq'::regclass);

ALTER TABLE ONLY public.operations_cloud_observations ALTER COLUMN id SET DEFAULT nextval('public.operations_cloud_observations_id_seq'::regclass);

ALTER TABLE ONLY public.operations_hetzner_observations ALTER COLUMN id SET DEFAULT nextval('public.operations_hetzner_observations_id_seq'::regclass);

ALTER TABLE ONLY public.operations_history ALTER COLUMN id SET DEFAULT nextval('public.operations_history_id_seq'::regclass);

ALTER TABLE ONLY public.operations_incident_deliveries ALTER COLUMN id SET DEFAULT nextval('public.operations_incident_deliveries_id_seq'::regclass);

ALTER TABLE ONLY public.operations_incident_notes ALTER COLUMN id SET DEFAULT nextval('public.operations_incident_notes_id_seq'::regclass);

ALTER TABLE ONLY public.server_alert_events ALTER COLUMN id SET DEFAULT nextval('public.server_offline_events_id_seq'::regclass);

ALTER TABLE ONLY public.server_traffic_corrections ALTER COLUMN id SET DEFAULT nextval('public.server_traffic_corrections_id_seq'::regclass);

ALTER TABLE ONLY public.servers ALTER COLUMN id SET DEFAULT nextval('public.servers_id_seq'::regclass);

ALTER TABLE ONLY public.singbox_chains ALTER COLUMN id SET DEFAULT nextval('public.singbox_chains_id_seq'::regclass);


ALTER TABLE ONLY public.singbox_deployment_preflights ALTER COLUMN sequence SET DEFAULT nextval('public.singbox_deployment_preflights_sequence_seq'::regclass);

ALTER TABLE ONLY public.singbox_invitation_quota_events ALTER COLUMN id SET DEFAULT nextval('public.singbox_invitation_quota_events_id_seq'::regclass);

ALTER TABLE ONLY public.singbox_member_content ALTER COLUMN id SET DEFAULT nextval('public.singbox_member_content_id_seq'::regclass);

ALTER TABLE ONLY public.singbox_member_messages ALTER COLUMN id SET DEFAULT nextval('public.singbox_member_messages_id_seq'::regclass);

ALTER TABLE ONLY public.singbox_nexus_nodes ALTER COLUMN id SET DEFAULT nextval('public.singbox_nexus_nodes_id_seq'::regclass);

ALTER TABLE ONLY public.singbox_nexus_usage_deliveries ALTER COLUMN id SET DEFAULT nextval('public.singbox_nexus_usage_deliveries_id_seq'::regclass);

ALTER TABLE ONLY public.singbox_operation_events ALTER COLUMN id SET DEFAULT nextval('public.singbox_operation_events_id_seq'::regclass);

ALTER TABLE ONLY public.singbox_ordered_subscription_sources ALTER COLUMN id SET DEFAULT nextval('public.singbox_ordered_subscription_sources_id_seq'::regclass);

ALTER TABLE ONLY public.singbox_package_assignments ALTER COLUMN id SET DEFAULT nextval('public.singbox_package_assignments_id_seq'::regclass);

ALTER TABLE ONLY public.singbox_package_groups ALTER COLUMN id SET DEFAULT nextval('public.singbox_package_groups_id_seq'::regclass);

ALTER TABLE ONLY public.singbox_payment_channels ALTER COLUMN id SET DEFAULT nextval('public.singbox_payment_channels_id_seq'::regclass);

ALTER TABLE ONLY public.singbox_policy_groups ALTER COLUMN id SET DEFAULT nextval('public.singbox_policy_groups_id_seq'::regclass);

ALTER TABLE ONLY public.singbox_reachability_deliveries ALTER COLUMN id SET DEFAULT nextval('public.singbox_reachability_deliveries_id_seq'::regclass);

ALTER TABLE ONLY public.singbox_reachability_history ALTER COLUMN id SET DEFAULT nextval('public.singbox_reachability_history_id_seq'::regclass);

ALTER TABLE ONLY public.singbox_reachability_targets ALTER COLUMN id SET DEFAULT nextval('public.singbox_reachability_targets_id_seq'::regclass);

ALTER TABLE ONLY public.singbox_subscription_source_revisions ALTER COLUMN generation SET DEFAULT nextval('public.singbox_subscription_source_revisions_generation_seq'::regclass);

ALTER TABLE ONLY public.system_observer_reports ALTER COLUMN id SET DEFAULT nextval('public.system_observer_reports_id_seq'::regclass);

ALTER TABLE ONLY public.users ALTER COLUMN id SET DEFAULT nextval('public.users_id_seq'::regclass);

ALTER TABLE ONLY public.accesses
    ADD CONSTRAINT accesses_pkey PRIMARY KEY (user_id, node_id);

ALTER TABLE ONLY public.accesses
    ADD CONSTRAINT accesses_stat_name_key UNIQUE (stat_name);

ALTER TABLE ONLY public.accesses
    ADD CONSTRAINT accesses_uuid_key UNIQUE (uuid);

ALTER TABLE ONLY public.administrator_passkey_accounts
    ADD CONSTRAINT administrator_passkey_accounts_account_id_key UNIQUE (account_id);

ALTER TABLE ONLY public.administrator_passkey_accounts
    ADD CONSTRAINT administrator_passkey_accounts_pkey PRIMARY KEY (admin_id);

ALTER TABLE ONLY public.administrator_preferences
    ADD CONSTRAINT administrator_preferences_pkey PRIMARY KEY (admin_id, preference_key);

ALTER TABLE ONLY public.administrator_profiles
    ADD CONSTRAINT administrator_profiles_login_name_key UNIQUE (login_name);

ALTER TABLE ONLY public.administrator_profiles
    ADD CONSTRAINT administrator_profiles_pkey PRIMARY KEY (admin_id);

ALTER TABLE ONLY public.administrator_reauth
    ADD CONSTRAINT administrator_reauth_pkey PRIMARY KEY (session_hash);

ALTER TABLE ONLY public.administrator_server_grants
    ADD CONSTRAINT administrator_server_grants_pkey PRIMARY KEY (admin_id, server_id);

ALTER TABLE ONLY public.admins
    ADD CONSTRAINT admins_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.alert_reminder_receipts
    ADD CONSTRAINT alert_reminder_receipts_pkey PRIMARY KEY (server_id, source_key);

ALTER TABLE ONLY public.alert_rules
    ADD CONSTRAINT alert_rules_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.alicloud_accounts
    ADD CONSTRAINT alicloud_accounts_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.alicloud_deliveries
    ADD CONSTRAINT alicloud_deliveries_event_id_channel_key UNIQUE (event_id, channel);

ALTER TABLE ONLY public.alicloud_deliveries
    ADD CONSTRAINT alicloud_deliveries_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.alicloud_eip_rotations
    ADD CONSTRAINT alicloud_eip_rotations_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.alicloud_events
    ADD CONSTRAINT alicloud_events_dedup_key_key UNIQUE (dedup_key);

ALTER TABLE ONLY public.alicloud_events
    ADD CONSTRAINT alicloud_events_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.alicloud_managed_security_groups
    ADD CONSTRAINT alicloud_managed_security_groups_pkey PRIMARY KEY (resource_id, group_id);

ALTER TABLE ONLY public.alicloud_operations
    ADD CONSTRAINT alicloud_operations_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.alicloud_power_jobs
    ADD CONSTRAINT alicloud_power_jobs_dedup_key_key UNIQUE (dedup_key);

ALTER TABLE ONLY public.alicloud_power_jobs
    ADD CONSTRAINT alicloud_power_jobs_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.alicloud_resources
    ADD CONSTRAINT alicloud_resources_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.alicloud_security_group_operations
    ADD CONSTRAINT alicloud_security_group_operations_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.alicloud_traffic_history
    ADD CONSTRAINT alicloud_traffic_history_pkey PRIMARY KEY (account_id, bucket_seconds, bucket_start);

ALTER TABLE ONLY public.auth_rate_limits
    ADD CONSTRAINT auth_rate_limits_pkey PRIMARY KEY (scope);

ALTER TABLE ONLY public.credential_entries
    ADD CONSTRAINT credential_entries_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.ddns_history
    ADD CONSTRAINT ddns_history_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.ddns_migration_previews
    ADD CONSTRAINT ddns_migration_previews_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.ddns_rules
    ADD CONSTRAINT ddns_rules_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.deployments
    ADD CONSTRAINT deployments_pkey PRIMARY KEY (server_id, module, rev);

ALTER TABLE ONLY public.diagnostic_jobs
    ADD CONSTRAINT diagnostic_jobs_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.diagnostic_report_sections
    ADD CONSTRAINT diagnostic_report_sections_pkey PRIMARY KEY (job_id, name);

ALTER TABLE ONLY public.dns_accounts
    ADD CONSTRAINT dns_accounts_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.dns_record_history
    ADD CONSTRAINT dns_record_history_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.dns_record_previews
    ADD CONSTRAINT dns_record_previews_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.dns_resolver_observations
    ADD CONSTRAINT dns_resolver_observations_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.enrollment_tokens
    ADD CONSTRAINT enrollment_tokens_pkey PRIMARY KEY (token_hash);

ALTER TABLE ONLY public.exchange_rates
    ADD CONSTRAINT exchange_rates_pkey PRIMARY KEY (singleton);

ALTER TABLE ONLY public.expense_deliveries
    ADD CONSTRAINT expense_deliveries_event_id_channel_key UNIQUE (event_id, channel);

ALTER TABLE ONLY public.expense_deliveries
    ADD CONSTRAINT expense_deliveries_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.expense_events
    ADD CONSTRAINT expense_events_event_key_key UNIQUE (event_key);

ALTER TABLE ONLY public.expense_events
    ADD CONSTRAINT expense_events_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.expense_imports
    ADD CONSTRAINT expense_imports_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.expense_imports
    ADD CONSTRAINT expense_imports_source_id_key UNIQUE (source_id);

ALTER TABLE ONLY public.expense_labels
    ADD CONSTRAINT expense_labels_kind_name_key UNIQUE (kind, name);

ALTER TABLE ONLY public.expense_labels
    ADD CONSTRAINT expense_labels_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.expense_payments
    ADD CONSTRAINT expense_payments_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.expense_payments
    ADD CONSTRAINT expense_payments_request_id_key UNIQUE (request_id);

ALTER TABLE ONLY public.expense_settings
    ADD CONSTRAINT expense_settings_pkey PRIMARY KEY (singleton);

ALTER TABLE ONLY public.expense_subscriptions
    ADD CONSTRAINT expense_subscriptions_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.fleet_config_history
    ADD CONSTRAINT fleet_config_history_operation_id_key UNIQUE (operation_id);

ALTER TABLE ONLY public.fleet_config_history
    ADD CONSTRAINT fleet_config_history_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.fleet_cost_records
    ADD CONSTRAINT fleet_cost_records_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.fleet_events
    ADD CONSTRAINT fleet_events_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.fleet_operations
    ADD CONSTRAINT fleet_operations_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.fleet_profiles
    ADD CONSTRAINT fleet_profiles_pkey PRIMARY KEY (server_id);

ALTER TABLE ONLY public.fleet_templates
    ADD CONSTRAINT fleet_templates_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.fleet_terminal_inputs
    ADD CONSTRAINT fleet_terminal_inputs_pkey PRIMARY KEY (session_id, sequence);

ALTER TABLE ONLY public.fleet_terminal_outputs
    ADD CONSTRAINT fleet_terminal_outputs_pkey PRIMARY KEY (session_id, sequence);

ALTER TABLE ONLY public.fleet_terminal_sessions
    ADD CONSTRAINT fleet_terminal_sessions_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.latency_tasks
    ADD CONSTRAINT latency_tasks_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.management_api_tokens
    ADD CONSTRAINT management_api_tokens_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.management_api_tokens
    ADD CONSTRAINT management_api_tokens_token_hash_key UNIQUE (token_hash);

ALTER TABLE ONLY public.management_audit
    ADD CONSTRAINT management_audit_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.metrics_minutely
    ADD CONSTRAINT metrics_minutely_pkey PRIMARY KEY (server_id, bucket);

ALTER TABLE ONLY public.network_acme_jobs
    ADD CONSTRAINT network_acme_jobs_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.network_acme_plans
    ADD CONSTRAINT network_acme_plans_pkey PRIMARY KEY (certificate_id);

ALTER TABLE ONLY public.network_certificate_deployment_previews
    ADD CONSTRAINT network_certificate_deployment_previews_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.network_certificate_deployments
    ADD CONSTRAINT network_certificate_deployments_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.network_certificate_versions
    ADD CONSTRAINT network_certificate_versions_certificate_id_revision_key UNIQUE (certificate_id, revision);

ALTER TABLE ONLY public.network_certificate_versions
    ADD CONSTRAINT network_certificate_versions_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.network_dns_challenges
    ADD CONSTRAINT network_dns_challenges_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.network_document_history
    ADD CONSTRAINT network_document_history_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.network_documents
    ADD CONSTRAINT network_documents_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.network_observations
    ADD CONSTRAINT network_observations_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.network_operation_links
    ADD CONSTRAINT network_operation_links_pkey PRIMARY KEY (operation_id);

ALTER TABLE ONLY public.network_probes
    ADD CONSTRAINT network_probes_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.network_server_migration_previews
    ADD CONSTRAINT network_server_migration_previews_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.network_workbench_ip_evidence
    ADD CONSTRAINT network_workbench_ip_evidence_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.network_workbench_pairings
    ADD CONSTRAINT network_workbench_pairings_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.network_workbench_plans
    ADD CONSTRAINT network_workbench_plans_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.network_workbench_providers
    ADD CONSTRAINT network_workbench_providers_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.network_workbench_results
    ADD CONSTRAINT network_workbench_results_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.network_workbench_results
    ADD CONSTRAINT network_workbench_results_run_id_step_index_role_key UNIQUE (run_id, step_index, role);

ALTER TABLE ONLY public.network_workbench_runs
    ADD CONSTRAINT network_workbench_runs_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.network_workbench_shares
    ADD CONSTRAINT network_workbench_shares_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.network_workbench_shares
    ADD CONSTRAINT network_workbench_shares_token_hash_key UNIQUE (token_hash);

ALTER TABLE ONLY public.network_workbench_targets
    ADD CONSTRAINT network_workbench_targets_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.network_workbench_tools
    ADD CONSTRAINT network_workbench_tools_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.nodes
    ADD CONSTRAINT nodes_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.notification_channel_tests
    ADD CONSTRAINT notification_channel_tests_pkey PRIMARY KEY (channel);

ALTER TABLE ONLY public.notification_outbox
    ADD CONSTRAINT notification_outbox_event_kind_channel_key UNIQUE (event_id, kind, channel);

ALTER TABLE ONLY public.notification_outbox
    ADD CONSTRAINT notification_outbox_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.notification_test_limit
    ADD CONSTRAINT notification_test_limit_pkey PRIMARY KEY (singleton);

ALTER TABLE ONLY public.operations_backup_records
    ADD CONSTRAINT operations_backup_records_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.operations_backup_schedules
    ADD CONSTRAINT operations_backup_schedules_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.operations_cancellation_reminders
    ADD CONSTRAINT operations_cancellation_reminders_pkey PRIMARY KEY (record_id);

ALTER TABLE ONLY public.operations_cloud_links
    ADD CONSTRAINT operations_cloud_links_pkey PRIMARY KEY (resource_id);

ALTER TABLE ONLY public.operations_cloud_observations
    ADD CONSTRAINT operations_cloud_observations_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.operations_cloud_observations
    ADD CONSTRAINT operations_cloud_observations_resource_id_checked_at_source_key UNIQUE (resource_id, checked_at, source);

ALTER TABLE ONLY public.operations_hetzner_accounts
    ADD CONSTRAINT operations_hetzner_accounts_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.operations_hetzner_observations
    ADD CONSTRAINT operations_hetzner_observations_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.operations_hetzner_observations
    ADD CONSTRAINT operations_hetzner_observations_resource_id_refresh_id_key UNIQUE (resource_id, refresh_id);

ALTER TABLE ONLY public.operations_hetzner_refreshes
    ADD CONSTRAINT operations_hetzner_refreshes_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.operations_hetzner_resources
    ADD CONSTRAINT operations_hetzner_resources_account_id_cloud_id_key UNIQUE (account_id, cloud_id);

ALTER TABLE ONLY public.operations_hetzner_resources
    ADD CONSTRAINT operations_hetzner_resources_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.operations_history
    ADD CONSTRAINT operations_history_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.operations_incident_deliveries
    ADD CONSTRAINT operations_incident_deliveries_incident_id_channel_kind_key UNIQUE (incident_id, channel, kind);

ALTER TABLE ONLY public.operations_incident_deliveries
    ADD CONSTRAINT operations_incident_deliveries_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.operations_incident_notes
    ADD CONSTRAINT operations_incident_notes_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.operations_incidents
    ADD CONSTRAINT operations_incidents_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.operations_jobs
    ADD CONSTRAINT operations_jobs_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.operations_jobs
    ADD CONSTRAINT operations_jobs_source_schedule_source_window_key UNIQUE (source_schedule, source_window);

ALTER TABLE ONLY public.operations_maintenance
    ADD CONSTRAINT operations_maintenance_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.operations_panel_steps
    ADD CONSTRAINT operations_panel_steps_execution_id_key UNIQUE (execution_id);

ALTER TABLE ONLY public.operations_panel_steps
    ADD CONSTRAINT operations_panel_steps_pkey PRIMARY KEY (job_id, "position");

ALTER TABLE ONLY public.operations_remediation_rules
    ADD CONSTRAINT operations_remediation_rules_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.operations_restore_drills
    ADD CONSTRAINT operations_restore_drills_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.operations_schedules
    ADD CONSTRAINT operations_schedules_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.operations_server_locks
    ADD CONSTRAINT operations_server_locks_pkey PRIMARY KEY (server_id);

ALTER TABLE ONLY public.operations_target_steps
    ADD CONSTRAINT operations_target_steps_fleet_operation_id_key UNIQUE (fleet_operation_id);

ALTER TABLE ONLY public.operations_target_steps
    ADD CONSTRAINT operations_target_steps_pkey PRIMARY KEY (job_id, server_id, "position");

ALTER TABLE ONLY public.operations_target_steps
    ADD CONSTRAINT operations_target_steps_runtime_operation_id_key UNIQUE (runtime_operation_id);

ALTER TABLE ONLY public.operations_templates
    ADD CONSTRAINT operations_templates_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.panel_settings
    ADD CONSTRAINT panel_settings_pkey PRIMARY KEY (singleton);

ALTER TABLE ONLY public.passkey_accounts
    ADD CONSTRAINT passkey_accounts_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.passkey_ceremonies
    ADD CONSTRAINT passkey_ceremonies_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.passkey_credentials
    ADD CONSTRAINT passkey_credentials_credential_id_key UNIQUE (credential_id);

ALTER TABLE ONLY public.passkey_credentials
    ADD CONSTRAINT passkey_credentials_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.probe_leases
    ADD CONSTRAINT probe_leases_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.probe_results
    ADD CONSTRAINT probe_results_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.record_retention_policy
    ADD CONSTRAINT record_retention_policy_pkey PRIMARY KEY (kind);

ALTER TABLE ONLY public.remote_commands
    ADD CONSTRAINT remote_commands_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.runtime_apply_facts
    ADD CONSTRAINT runtime_apply_facts_pkey PRIMARY KEY (server_id, op_id);

ALTER TABLE ONLY public.runtime_control_devices
    ADD CONSTRAINT runtime_control_devices_pkey PRIMARY KEY (server_id);

ALTER TABLE ONLY public.runtime_control_receipts
    ADD CONSTRAINT runtime_control_receipts_pkey PRIMARY KEY (request_id);

ALTER TABLE ONLY public.runtime_control_requests
    ADD CONSTRAINT runtime_control_requests_pkey PRIMARY KEY (request_id);

ALTER TABLE ONLY public.runtime_deployment_bindings
    ADD CONSTRAINT runtime_deployment_bindings_pkey PRIMARY KEY (deployment_id);

ALTER TABLE ONLY public.runtime_deployment_bindings
    ADD CONSTRAINT runtime_deployment_bindings_server_id_module_rev_key UNIQUE (server_id, module, rev);

ALTER TABLE ONLY public.runtime_enablements
    ADD CONSTRAINT runtime_enablements_pkey PRIMARY KEY (server_id, module);

ALTER TABLE ONLY public.runtime_module_checkpoints
    ADD CONSTRAINT runtime_module_checkpoints_pkey PRIMARY KEY (server_id, module);

ALTER TABLE ONLY public.runtime_operations
    ADD CONSTRAINT runtime_operations_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.runtime_validations
    ADD CONSTRAINT runtime_validations_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.server_ip_quality_datasets
    ADD CONSTRAINT server_ip_quality_datasets_pkey PRIMARY KEY (server_id, ip, provider, database);

ALTER TABLE ONLY public.server_ip_quality_node_generations
    ADD CONSTRAINT server_ip_quality_node_generations_pkey PRIMARY KEY (server_id, ip_version);

ALTER TABLE ONLY public.server_ip_quality
    ADD CONSTRAINT server_ip_quality_pkey PRIMARY KEY (server_id, ip, provider);

ALTER TABLE ONLY public.server_module_status
    ADD CONSTRAINT server_module_status_pkey PRIMARY KEY (server_id, module);

ALTER TABLE ONLY public.server_network_counters
    ADD CONSTRAINT server_network_counters_pkey PRIMARY KEY (server_id);

ALTER TABLE ONLY public.server_network_daily
    ADD CONSTRAINT server_network_daily_pkey PRIMARY KEY (server_id, day, interface);

ALTER TABLE ONLY public.server_alert_events
    ADD CONSTRAINT server_offline_events_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.server_retirements
    ADD CONSTRAINT server_retirements_pkey PRIMARY KEY (server_id);

ALTER TABLE ONLY public.server_retirements
    ADD CONSTRAINT server_retirements_request_id_key UNIQUE (request_id);

ALTER TABLE ONLY public.server_traffic_corrections
    ADD CONSTRAINT server_traffic_corrections_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.servers
    ADD CONSTRAINT servers_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.sessions
    ADD CONSTRAINT sessions_pkey PRIMARY KEY (token_hash);

ALTER TABLE ONLY public.singbox_account_challenges
    ADD CONSTRAINT singbox_account_challenges_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.singbox_account_limits
    ADD CONSTRAINT singbox_account_limits_pkey PRIMARY KEY (bucket);

ALTER TABLE ONLY public.singbox_chain_runtime_requirements
    ADD CONSTRAINT singbox_chain_runtime_requirements_pkey PRIMARY KEY (chain_id, generation, server_id);

ALTER TABLE ONLY public.singbox_chain_state
    ADD CONSTRAINT singbox_chain_state_pkey PRIMARY KEY (chain_id);

ALTER TABLE ONLY public.singbox_chains
    ADD CONSTRAINT singbox_chains_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.singbox_client_templates
    ADD CONSTRAINT singbox_client_templates_pkey PRIMARY KEY (user_id);

ALTER TABLE ONLY public.singbox_credential_rotations
    ADD CONSTRAINT singbox_credential_rotations_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.singbox_deployment_preflights
    ADD CONSTRAINT singbox_deployment_preflights_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.singbox_deployment_preflights
    ADD CONSTRAINT singbox_deployment_preflights_sequence_key UNIQUE (sequence);

ALTER TABLE ONLY public.singbox_deployment_projections
    ADD CONSTRAINT singbox_deployment_projections_pkey PRIMARY KEY (server_id, rev);

ALTER TABLE ONLY public.singbox_deployment_public_projection
    ADD CONSTRAINT singbox_deployment_public_projection_pkey PRIMARY KEY (server_id, module, revision, node_id);

ALTER TABLE ONLY public.singbox_entitlement_state
    ADD CONSTRAINT singbox_entitlement_state_pkey PRIMARY KEY (user_id);

ALTER TABLE ONLY public.singbox_installation
    ADD CONSTRAINT singbox_installation_pkey PRIMARY KEY (server_id);

ALTER TABLE ONLY public.singbox_invitation_policy_groups
    ADD CONSTRAINT singbox_invitation_policy_groups_pkey PRIMARY KEY (group_id);

ALTER TABLE ONLY public.singbox_invitation_quota_events
    ADD CONSTRAINT singbox_invitation_quota_events_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.singbox_invitation_quotas
    ADD CONSTRAINT singbox_invitation_quotas_pkey PRIMARY KEY (user_id);

ALTER TABLE ONLY public.singbox_invitation_settings
    ADD CONSTRAINT singbox_invitation_settings_pkey PRIMARY KEY (singleton);

ALTER TABLE ONLY public.singbox_invitations
    ADD CONSTRAINT singbox_invitations_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.singbox_invitations
    ADD CONSTRAINT singbox_invitations_recipient_id_key UNIQUE (recipient_id);

ALTER TABLE ONLY public.singbox_invitations
    ADD CONSTRAINT singbox_invitations_token_hash_key UNIQUE (token_hash);

ALTER TABLE ONLY public.singbox_mail_settings
    ADD CONSTRAINT singbox_mail_settings_pkey PRIMARY KEY (singleton);

ALTER TABLE ONLY public.singbox_managed_endpoint_versions
    ADD CONSTRAINT singbox_managed_endpoint_versions_node_id_semantic_sha256_key UNIQUE (node_id, semantic_sha256);

ALTER TABLE ONLY public.singbox_managed_endpoint_versions
    ADD CONSTRAINT singbox_managed_endpoint_versions_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.singbox_member_content
    ADD CONSTRAINT singbox_member_content_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.singbox_member_messages
    ADD CONSTRAINT singbox_member_messages_event_key_key UNIQUE (event_key);

ALTER TABLE ONLY public.singbox_member_messages
    ADD CONSTRAINT singbox_member_messages_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.singbox_members
    ADD CONSTRAINT singbox_members_email_key UNIQUE (email);

ALTER TABLE ONLY public.singbox_members
    ADD CONSTRAINT singbox_members_pkey PRIMARY KEY (user_id);

ALTER TABLE ONLY public.singbox_nexus_bridge
    ADD CONSTRAINT singbox_nexus_bridge_pkey PRIMARY KEY (singleton);

ALTER TABLE ONLY public.singbox_nexus_device_state
    ADD CONSTRAINT singbox_nexus_device_state_pkey PRIMARY KEY (user_id);

ALTER TABLE ONLY public.singbox_nexus_identities
    ADD CONSTRAINT singbox_nexus_identities_pkey PRIMARY KEY (user_id);

ALTER TABLE ONLY public.singbox_nexus_imports
    ADD CONSTRAINT singbox_nexus_imports_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.singbox_nexus_imports
    ADD CONSTRAINT singbox_nexus_imports_source_id_key UNIQUE (source_id);

ALTER TABLE ONLY public.singbox_nexus_metering
    ADD CONSTRAINT singbox_nexus_metering_pkey PRIMARY KEY (user_id);

ALTER TABLE ONLY public.singbox_nexus_node_imports
    ADD CONSTRAINT singbox_nexus_node_imports_pkey PRIMARY KEY (source_id);

ALTER TABLE ONLY public.singbox_nexus_node_status
    ADD CONSTRAINT singbox_nexus_node_status_pkey PRIMARY KEY (legacy_node_id);

ALTER TABLE ONLY public.singbox_nexus_nodes
    ADD CONSTRAINT singbox_nexus_nodes_credential_id_key UNIQUE (credential_id);

ALTER TABLE ONLY public.singbox_nexus_nodes
    ADD CONSTRAINT singbox_nexus_nodes_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.singbox_nexus_nodes
    ADD CONSTRAINT singbox_nexus_nodes_source_id_legacy_node_id_key UNIQUE (source_id, legacy_node_id);

ALTER TABLE ONLY public.singbox_nexus_plan_mappings
    ADD CONSTRAINT singbox_nexus_plan_mappings_pkey PRIMARY KEY (package_group_id);

ALTER TABLE ONLY public.singbox_nexus_runtime_hosts
    ADD CONSTRAINT singbox_nexus_runtime_hosts_credential_id_key UNIQUE (credential_id);

ALTER TABLE ONLY public.singbox_nexus_runtime_hosts
    ADD CONSTRAINT singbox_nexus_runtime_hosts_pkey PRIMARY KEY (server_id);

ALTER TABLE ONLY public.singbox_nexus_runtime_nodes
    ADD CONSTRAINT singbox_nexus_runtime_nodes_node_id_key UNIQUE (node_id);

ALTER TABLE ONLY public.singbox_nexus_runtime_nodes
    ADD CONSTRAINT singbox_nexus_runtime_nodes_pkey PRIMARY KEY (inventory_id);

ALTER TABLE ONLY public.singbox_nexus_usage_deliveries
    ADD CONSTRAINT singbox_nexus_usage_deliverie_server_id_epoch_seq_stat_name_key UNIQUE (server_id, epoch, seq, stat_name);

ALTER TABLE ONLY public.singbox_nexus_usage_deliveries
    ADD CONSTRAINT singbox_nexus_usage_deliveries_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.singbox_node_metadata
    ADD CONSTRAINT singbox_node_metadata_pkey PRIMARY KEY (kind, id);

ALTER TABLE ONLY public.singbox_operation_events
    ADD CONSTRAINT singbox_operation_events_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.singbox_operation_previews
    ADD CONSTRAINT singbox_operation_previews_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.singbox_operation_privacy
    ADD CONSTRAINT singbox_operation_privacy_pkey PRIMARY KEY (singleton);

ALTER TABLE ONLY public.singbox_ordered_chain_creation_requests
    ADD CONSTRAINT singbox_ordered_chain_creation_requests_pkey PRIMARY KEY (request_id);

ALTER TABLE ONLY public.singbox_ordered_chain_hops
    ADD CONSTRAINT singbox_ordered_chain_hops_pkey PRIMARY KEY (chain_id, generation, "position");

ALTER TABLE ONLY public.singbox_ordered_chain_hops
    ADD CONSTRAINT singbox_ordered_chain_hops_relay_uuid_key UNIQUE (relay_uuid);

ALTER TABLE ONLY public.singbox_ordered_chain_versions
    ADD CONSTRAINT singbox_ordered_chain_versions_pkey PRIMARY KEY (chain_id, generation);

ALTER TABLE ONLY public.singbox_ordered_external_nodes
    ADD CONSTRAINT singbox_ordered_external_node_public_id UNIQUE (public_id);

ALTER TABLE ONLY public.singbox_ordered_external_node_versions
    ADD CONSTRAINT singbox_ordered_external_node_versions_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.singbox_ordered_external_nodes
    ADD CONSTRAINT singbox_ordered_external_nodes_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.singbox_ordered_external_node_versions
    ADD CONSTRAINT singbox_ordered_external_version_public_id UNIQUE (public_id);

ALTER TABLE ONLY public.singbox_ordered_source_previews
    ADD CONSTRAINT singbox_ordered_source_previews_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.singbox_ordered_subscription_sources
    ADD CONSTRAINT singbox_ordered_subscription_sources_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.singbox_package_assignments
    ADD CONSTRAINT singbox_package_assignments_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.singbox_package_assignments
    ADD CONSTRAINT singbox_package_assignments_user_id_id_key UNIQUE (user_id, id);

ALTER TABLE ONLY public.singbox_package_assignments
    ADD CONSTRAINT singbox_package_assignments_user_id_request_id_key UNIQUE (user_id, request_id);

ALTER TABLE ONLY public.singbox_package_groups
    ADD CONSTRAINT singbox_package_groups_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.singbox_path_controller_secrets
    ADD CONSTRAINT singbox_path_controller_secrets_pkey PRIMARY KEY (server_id);

ALTER TABLE ONLY public.singbox_path_deployment_dependencies
    ADD CONSTRAINT singbox_path_deployment_dependencies_pkey PRIMARY KEY (server_id, module, revision, chain_id, generation, hop_position);

ALTER TABLE ONLY public.singbox_path_probes
    ADD CONSTRAINT singbox_path_probes_chain_id_generation_stage_key UNIQUE (chain_id, generation, stage);

ALTER TABLE ONLY public.singbox_path_probes
    ADD CONSTRAINT singbox_path_probes_pkey PRIMARY KEY (probe_id);

ALTER TABLE ONLY public.singbox_path_stage_deployments
    ADD CONSTRAINT singbox_path_stage_deployments_pkey PRIMARY KEY (chain_id, generation, stage, server_id);

ALTER TABLE ONLY public.singbox_payment_channels
    ADD CONSTRAINT singbox_payment_channels_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.singbox_policy_chains
    ADD CONSTRAINT singbox_policy_chains_pkey PRIMARY KEY (group_id, chain_id);

ALTER TABLE ONLY public.singbox_policy_external_nodes
    ADD CONSTRAINT singbox_policy_external_nodes_pkey PRIMARY KEY (group_id, external_node_id);

ALTER TABLE ONLY public.singbox_policy_groups
    ADD CONSTRAINT singbox_policy_groups_legacy_group_id_key UNIQUE (legacy_group_id);

ALTER TABLE ONLY public.singbox_policy_groups
    ADD CONSTRAINT singbox_policy_groups_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.singbox_policy_nodes
    ADD CONSTRAINT singbox_policy_nodes_pkey PRIMARY KEY (group_id, node_id);

ALTER TABLE ONLY public.singbox_portal_accounts
    ADD CONSTRAINT singbox_portal_accounts_account_id_key UNIQUE (account_id);

ALTER TABLE ONLY public.singbox_portal_accounts
    ADD CONSTRAINT singbox_portal_accounts_activation_hash_key UNIQUE (activation_hash);

ALTER TABLE ONLY public.singbox_portal_accounts
    ADD CONSTRAINT singbox_portal_accounts_pkey PRIMARY KEY (user_id);

ALTER TABLE ONLY public.singbox_portal_sessions
    ADD CONSTRAINT singbox_portal_sessions_pkey PRIMARY KEY (token_hash);

ALTER TABLE ONLY public.singbox_quota_credits
    ADD CONSTRAINT singbox_quota_credits_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.singbox_reachability_deliveries
    ADD CONSTRAINT singbox_reachability_deliveries_event_id_channel_key UNIQUE (event_id, channel);

ALTER TABLE ONLY public.singbox_reachability_deliveries
    ADD CONSTRAINT singbox_reachability_deliveries_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.singbox_reachability_events
    ADD CONSTRAINT singbox_reachability_events_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.singbox_reachability_history
    ADD CONSTRAINT singbox_reachability_history_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.singbox_reachability_settings
    ADD CONSTRAINT singbox_reachability_settings_pkey PRIMARY KEY (singleton);

ALTER TABLE ONLY public.singbox_reachability_targets
    ADD CONSTRAINT singbox_reachability_targets_host_port_key UNIQUE (host, port);

ALTER TABLE ONLY public.singbox_reachability_targets
    ADD CONSTRAINT singbox_reachability_targets_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.singbox_resource_mutation_requests
    ADD CONSTRAINT singbox_resource_mutation_requests_pkey PRIMARY KEY (request_id);

ALTER TABLE ONLY public.singbox_runtime_manifest_facts
    ADD CONSTRAINT singbox_runtime_manifest_facts_pkey PRIMARY KEY (server_id, module, revision);

ALTER TABLE ONLY public.singbox_runtime_rollout_members
    ADD CONSTRAINT singbox_runtime_rollout_members_pkey PRIMARY KEY (rollout_id, server_id);

ALTER TABLE ONLY public.singbox_runtime_rollouts
    ADD CONSTRAINT singbox_runtime_rollouts_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.singbox_runtime_session_addresses
    ADD CONSTRAINT singbox_runtime_session_addresses_pkey PRIMARY KEY (server_id, stat_name, address);

ALTER TABLE ONLY public.singbox_subscription_domain
    ADD CONSTRAINT singbox_subscription_domain_pkey PRIMARY KEY (singleton);

ALTER TABLE ONLY public.singbox_subscription_host_retired
    ADD CONSTRAINT singbox_subscription_host_retired_pkey PRIMARY KEY (record_id);

ALTER TABLE ONLY public.singbox_subscription_hosts
    ADD CONSTRAINT singbox_subscription_hosts_path_token_key UNIQUE (path_token);

ALTER TABLE ONLY public.singbox_subscription_hosts
    ADD CONSTRAINT singbox_subscription_hosts_pkey PRIMARY KEY (user_id);

ALTER TABLE ONLY public.singbox_subscription_hosts
    ADD CONSTRAINT singbox_subscription_hosts_slug_key UNIQUE (slug);

ALTER TABLE ONLY public.singbox_subscription_revision_nodes
    ADD CONSTRAINT singbox_subscription_revision_no_source_revision_id_node_id_key UNIQUE (source_revision_id, node_id);

ALTER TABLE ONLY public.singbox_subscription_revision_nodes
    ADD CONSTRAINT singbox_subscription_revision_nodes_pkey PRIMARY KEY (source_revision_id, ordinal);

ALTER TABLE ONLY public.singbox_subscription_source_jobs
    ADD CONSTRAINT singbox_subscription_source_jobs_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.singbox_subscription_source_requests
    ADD CONSTRAINT singbox_subscription_source_requests_pkey PRIMARY KEY (request_id);

ALTER TABLE ONLY public.singbox_subscription_source_revisions
    ADD CONSTRAINT singbox_subscription_source_revisions_generation_key UNIQUE (generation);

ALTER TABLE ONLY public.singbox_subscription_source_revisions
    ADD CONSTRAINT singbox_subscription_source_revisions_job_id_key UNIQUE (job_id);

ALTER TABLE ONLY public.singbox_subscription_source_revisions
    ADD CONSTRAINT singbox_subscription_source_revisions_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.singbox_usage_daily
    ADD CONSTRAINT singbox_usage_daily_pkey PRIMARY KEY (user_id, day, node_id);

ALTER TABLE ONLY public.singbox_user_packages
    ADD CONSTRAINT singbox_user_packages_pkey PRIMARY KEY (user_id);

ALTER TABLE ONLY public.singbox_user_policies
    ADD CONSTRAINT singbox_user_policies_pkey PRIMARY KEY (user_id, group_id);

ALTER TABLE ONLY public.singbox_wallets
    ADD CONSTRAINT singbox_wallets_pkey PRIMARY KEY (user_id);

ALTER TABLE ONLY public.system_observer_reports
    ADD CONSTRAINT system_observer_reports_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.system_worker_heartbeats
    ADD CONSTRAINT system_worker_heartbeats_pkey PRIMARY KEY (name);

ALTER TABLE ONLY public.tcpquality_target_regions
    ADD CONSTRAINT tcpquality_target_regions_pkey PRIMARY KEY (probe_id);

ALTER TABLE ONLY public.telemetry_history_initialized
    ADD CONSTRAINT telemetry_history_initialized_pkey PRIMARY KEY (server_id);

ALTER TABLE ONLY public.telemetry_history
    ADD CONSTRAINT telemetry_history_pkey PRIMARY KEY (server_id, resolution_secs, bucket_at);

ALTER TABLE ONLY public.telemetry_policy
    ADD CONSTRAINT telemetry_policy_pkey PRIMARY KEY (singleton);

ALTER TABLE ONLY public.telemetry_receipts
    ADD CONSTRAINT telemetry_receipts_pkey PRIMARY KEY (server_id, id);

ALTER TABLE ONLY public.telemetry_samples
    ADD CONSTRAINT telemetry_samples_pkey PRIMARY KEY (server_id, id);

ALTER TABLE ONLY public.tool_advisory_observations
    ADD CONSTRAINT tool_advisory_observations_pkey PRIMARY KEY (ecosystem, name, version);

ALTER TABLE ONLY public.usage_batches
    ADD CONSTRAINT usage_batches_pkey PRIMARY KEY (server_id, epoch, seq);

ALTER TABLE ONLY public.usage_records
    ADD CONSTRAINT usage_records_server_id_epoch_seq_stat_name_key UNIQUE (server_id, epoch, seq, stat_name);

ALTER TABLE ONLY public.users
    ADD CONSTRAINT users_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.users
    ADD CONSTRAINT users_subscription_token_key UNIQUE (subscription_token);

CREATE INDEX accesses_node_idx ON public.accesses USING btree (node_id);

CREATE UNIQUE INDEX alicloud_automatic_cycle ON public.alicloud_operations USING btree (resource_id, billing_cycle, account_revision, resource_revision) WHERE (source = 'automatic'::text);

CREATE UNIQUE INDEX alicloud_eip_rotation_active ON public.alicloud_eip_rotations USING btree (resource_id) WHERE (status = ANY (ARRAY['running'::text, 'unknown'::text]));

CREATE INDEX alicloud_eip_rotation_due ON public.alicloud_eip_rotations USING btree (next_check_at) WHERE (status = 'running'::text);

CREATE INDEX alicloud_eip_rotation_history ON public.alicloud_eip_rotations USING btree (resource_id, created_at DESC);

CREATE UNIQUE INDEX alicloud_operation_active ON public.alicloud_operations USING btree (resource_id) WHERE (status = ANY (ARRAY['queued'::text, 'running'::text, 'uncertain'::text]));

CREATE INDEX alicloud_operation_history ON public.alicloud_operations USING btree (created_at DESC);

CREATE UNIQUE INDEX alicloud_power_active ON public.alicloud_power_jobs USING btree (resource_id) WHERE (status = ANY (ARRAY['queued'::text, 'running'::text, 'uncertain'::text]));

CREATE INDEX alicloud_power_pending ON public.alicloud_power_jobs USING btree (next_check_at, created_at) WHERE (status = ANY (ARRAY['queued'::text, 'running'::text, 'uncertain'::text]));

CREATE UNIQUE INDEX alicloud_resource_identity ON public.alicloud_resources USING btree (kind, region, cloud_id) WHERE (NOT archived);

CREATE UNIQUE INDEX alicloud_security_group_active ON public.alicloud_security_group_operations USING btree (resource_id) WHERE (status = ANY (ARRAY['running'::text, 'unknown'::text]));

CREATE INDEX alicloud_security_group_history ON public.alicloud_security_group_operations USING btree (resource_id, created_at DESC);

CREATE INDEX alicloud_traffic_history_retention ON public.alicloud_traffic_history USING btree (bucket_start);

CREATE INDEX command_pending_state_idx ON public.remote_commands USING btree (server_id, state, requested_at);

CREATE INDEX command_server_idx ON public.remote_commands USING btree (server_id, requested_at);

CREATE INDEX ddns_due ON public.ddns_rules USING btree (next_run_at);

CREATE INDEX ddns_history_latest ON public.ddns_history USING btree (rule_id, occurred_at DESC);

CREATE UNIQUE INDEX ddns_record_identity ON public.ddns_rules USING btree (COALESCE((config ->> 'provider'::text), 'cloudflare'::text), ((config ->> 'zone_id'::text)), ((config ->> 'record_name'::text)), ((config ->> 'record_type'::text)), COALESCE((config ->> 'line'::text), ''::text));

CREATE UNIQUE INDEX diagnostic_active_server_idx ON public.diagnostic_jobs USING btree (server_id) WHERE (status = ANY (ARRAY['queued'::text, 'running'::text, 'cleaning'::text]));

CREATE INDEX diagnostic_cancel_pending_idx ON public.diagnostic_jobs USING btree (server_id, cancel_requested_at) WHERE (status = 'cancel_requested'::text);

CREATE INDEX diagnostic_expiry_idx ON public.diagnostic_jobs USING btree (expires_at) WHERE (status = ANY (ARRAY['queued'::text, 'running'::text]));

CREATE INDEX diagnostic_history_idx ON public.diagnostic_jobs USING btree (server_id, created_at DESC);

CREATE INDEX dns_record_history_account ON public.dns_record_history USING btree (account_id, occurred_at DESC);

CREATE UNIQUE INDEX dns_record_history_preview_intent ON public.dns_record_history USING btree (preview_id) WHERE (preview_id IS NOT NULL);

CREATE UNIQUE INDEX dns_record_history_rollback_intent ON public.dns_record_history USING btree (rollback_of) WHERE (rollback_of IS NOT NULL);

CREATE INDEX dns_record_history_unresolved_account ON public.dns_record_history USING btree (account_id) WHERE (status = ANY (ARRAY['unknown'::text, 'submitted'::text, 'observed'::text]));

CREATE INDEX dns_resolver_observations_account ON public.dns_resolver_observations USING btree (account_id, occurred_at DESC);

CREATE INDEX enrollment_server_idx ON public.enrollment_tokens USING btree (server_id);

CREATE INDEX expense_payments_subscription ON public.expense_payments USING btree (subscription_id, paid_on DESC);

CREATE INDEX fleet_events_server_idx ON public.fleet_events USING btree (server_id, occurred_at DESC);

CREATE INDEX fleet_operations_pending_idx ON public.fleet_operations USING btree (server_id, requested_at) WHERE (status = 'queued'::text);

CREATE INDEX fleet_terminal_sessions_open_idx ON public.fleet_terminal_sessions USING btree (server_id) WHERE (status = ANY (ARRAY['queued'::text, 'running'::text]));

CREATE UNIQUE INDEX latency_task_server ON public.network_probes USING btree (task_id, server_id) WHERE (task_id IS NOT NULL);

CREATE INDEX management_api_tokens_expiry ON public.management_api_tokens USING btree (expires_at);

CREATE INDEX management_audit_time ON public.management_audit USING btree (occurred_at);

CREATE INDEX metrics_retention_idx ON public.metrics_minutely USING btree (bucket);

CREATE UNIQUE INDEX network_acme_single_active ON public.network_acme_jobs USING btree (certificate_id) WHERE (status = ANY (ARRAY['queued'::text, 'running'::text, 'unknown'::text]));

CREATE INDEX network_certificate_deployments_latest ON public.network_certificate_deployments USING btree (certificate_id, created_at DESC);

CREATE INDEX network_document_history_latest ON public.network_document_history USING btree (document_id, occurred_at DESC);

CREATE INDEX network_documents_kind ON public.network_documents USING btree (kind, updated_at DESC);

CREATE UNIQUE INDEX network_domain_name ON public.network_documents USING btree (((config ->> 'name'::text))) WHERE (kind = 'domain'::text);

CREATE UNIQUE INDEX network_managed_listener ON public.network_documents USING btree (((config ->> 'server_id'::text)), ((config ->> 'listen_address'::text)), ((config ->> 'listen_port'::text)), ((config ->> 'protocol'::text))) WHERE ((kind = 'forwarding'::text) AND ((config ->> 'owner'::text) = 'apsp'::text) AND ((config ->> 'enabled'::text) = 'true'::text));

CREATE INDEX network_observations_latest ON public.network_observations USING btree (document_id, observed_at DESC);

CREATE INDEX network_operation_links_document ON public.network_operation_links USING btree (document_id, created_at DESC);

CREATE INDEX network_server_migration_latest ON public.network_server_migration_previews USING btree (source_server_id, created_at DESC);

CREATE INDEX network_workbench_active_runs ON public.network_workbench_runs USING btree (status, created_at);

CREATE UNIQUE INDEX nodes_active_port_idx ON public.nodes USING btree (server_id, port) WHERE (deleted_at IS NULL);

CREATE INDEX notification_channel_pending ON public.notification_outbox USING btree (channel, next_attempt_at, id) WHERE (status = 'pending'::text);

CREATE INDEX notification_pending ON public.notification_outbox USING btree (next_attempt_at, id) WHERE (status = 'pending'::text);

CREATE INDEX operations_cloud_observation_history ON public.operations_cloud_observations USING btree (resource_id, checked_at DESC);

CREATE INDEX operations_hetzner_observation_history ON public.operations_hetzner_observations USING btree (resource_id, observed_at DESC);

CREATE INDEX operations_hetzner_resource_scope ON public.operations_hetzner_resources USING btree (server_id, account_id);

CREATE UNIQUE INDEX operations_incident_active ON public.operations_incidents USING btree (source_key) WHERE (status <> 'resolved'::text);

CREATE INDEX operations_maintenance_window ON public.operations_maintenance USING btree (starts_at, ends_at);

CREATE INDEX passkey_ceremonies_account ON public.passkey_ceremonies USING btree (account_id);

CREATE INDEX passkey_ceremonies_expiry ON public.passkey_ceremonies USING btree (expires_at);

CREATE INDEX passkey_credentials_account ON public.passkey_credentials USING btree (account_id);

CREATE INDEX probe_lease_server_time_idx ON public.probe_leases USING btree (server_id, issued_at DESC, id DESC);

CREATE INDEX probe_result_target_time_idx ON public.probe_results USING btree (server_id, probe_id, sampled_at DESC, id DESC);

CREATE INDEX probe_result_time_idx ON public.probe_results USING btree (server_id, sampled_at);

CREATE INDEX probe_server_idx ON public.network_probes USING btree (server_id);

CREATE INDEX runtime_control_checkpoint_revision_idx ON public.runtime_control_requests USING btree (server_id, (((request_json -> 'expected'::text) ->> 'revision'::text)::bigint)) WHERE (kind = 'checkpoint'::text);

CREATE INDEX runtime_control_expired_barrier_idx ON public.runtime_control_requests USING btree (server_id, created_at) WHERE ((kind = 'barrier'::text) AND (state = 'expired'::text));

CREATE UNIQUE INDEX runtime_control_pending_idx ON public.runtime_control_requests USING btree (server_id, module, kind) WHERE (state = 'pending'::text);

CREATE INDEX runtime_control_replay_idx ON public.runtime_control_requests USING btree (server_id, created_at) WHERE (state = 'pending'::text);

CREATE INDEX runtime_operations_history ON public.runtime_operations USING btree (server_id, requested_at DESC);

CREATE UNIQUE INDEX runtime_operations_one_active ON public.runtime_operations USING btree (server_id, module) WHERE ((result IS NULL) AND (cancelled_at IS NULL) AND (reconciled_at IS NULL));

CREATE INDEX runtime_validations_attempts ON public.runtime_validations USING btree (server_id, module, scope, generation, operation, revision, requested_at DESC);

CREATE INDEX runtime_validations_pending ON public.runtime_validations USING btree (server_id, requested_at) WHERE (result IS NULL);

CREATE INDEX server_alert_history ON public.server_alert_events USING btree (category, id DESC);

CREATE INDEX server_network_daily_day_idx ON public.server_network_daily USING btree (day, server_id);

CREATE INDEX server_node_ip_quality_history_idx ON public.server_ip_quality USING btree (server_id, checked_at DESC, ip, provider) WHERE (provider ~~ 'ipquality-node/%'::text);

CREATE UNIQUE INDEX server_one_active_alert ON public.server_alert_events USING btree (server_id, source_key) WHERE (resolved_at IS NULL);

CREATE INDEX server_traffic_correction_lookup ON public.server_traffic_corrections USING btree (server_id, cycle_start, reset_day, network_interface, id DESC);

CREATE INDEX sessions_expiry_idx ON public.sessions USING btree (expires_at);

CREATE INDEX singbox_chain_external_refs ON public.singbox_ordered_chain_hops USING btree (external_node_id, chain_id, generation);

CREATE INDEX singbox_chain_managed_refs ON public.singbox_ordered_chain_hops USING btree (managed_node_id, chain_id, generation);

CREATE UNIQUE INDEX singbox_chain_one_live_entry ON public.singbox_chains USING btree (entry_node_id) WHERE ((deleted_at IS NULL) OR ((path_kind = 'ordered'::text) AND (phase <> 'retired'::text)));

CREATE INDEX singbox_chain_source_refs ON public.singbox_ordered_chain_hops USING btree (source_id, chain_id, generation);

CREATE INDEX singbox_deployment_preflights_latest ON public.singbox_deployment_preflights USING btree (server_id, sequence DESC);

CREATE UNIQUE INDEX singbox_external_nodes_identity_idx ON public.singbox_ordered_external_nodes USING btree (source_id, identity_epoch, identity_key) WHERE (identity_state = 'unique'::text);

CREATE INDEX singbox_external_nodes_source_idx ON public.singbox_ordered_external_nodes USING btree (source_id, identity_epoch, id);

CREATE INDEX singbox_invitation_quota_events_user ON public.singbox_invitation_quota_events USING btree (user_id, id DESC);

CREATE INDEX singbox_invitations_issuer ON public.singbox_invitations USING btree (issuer_id, created_at DESC);

CREATE UNIQUE INDEX singbox_invitations_request ON public.singbox_invitations USING btree (issuer_id, request_id) WHERE (request_id IS NOT NULL);

CREATE UNIQUE INDEX singbox_live_chain_entry ON public.singbox_chains USING btree (entry_node_id) WHERE (deleted_at IS NULL);

CREATE INDEX singbox_member_messages_owner ON public.singbox_member_messages USING btree (user_id, id DESC);

CREATE INDEX singbox_nexus_nodes_server ON public.singbox_nexus_nodes USING btree (server_id);

CREATE INDEX singbox_nexus_usage_pending ON public.singbox_nexus_usage_deliveries USING btree (id) WHERE (delivered_at IS NULL);

CREATE INDEX singbox_nexus_usage_user_period ON public.singbox_nexus_usage_deliveries USING btree (user_id, period_end);

CREATE INDEX singbox_operation_events_user ON public.singbox_operation_events USING btree (user_id, created_at DESC);

CREATE INDEX singbox_operation_previews_expiry ON public.singbox_operation_previews USING btree (expires_at);

CREATE INDEX singbox_ordered_source_preview_expiry ON public.singbox_ordered_source_previews USING btree (expires_at);

CREATE INDEX singbox_path_dependency_lookup ON public.singbox_path_deployment_dependencies USING btree (chain_id, generation, server_id);

CREATE INDEX singbox_policy_external_node ON public.singbox_policy_external_nodes USING btree (external_node_id);

CREATE INDEX singbox_portal_sessions_account ON public.singbox_portal_sessions USING btree (account_id);

CREATE INDEX singbox_portal_sessions_expiry ON public.singbox_portal_sessions USING btree (expires_at);

CREATE INDEX singbox_quota_credits_cycle ON public.singbox_quota_credits USING btree (user_id, cycle_start, next_reset);

CREATE INDEX singbox_reachability_deliveries_pending ON public.singbox_reachability_deliveries USING btree (next_attempt_at, id) WHERE (state = ANY (ARRAY['pending'::text, 'sending'::text, 'failed'::text]));

CREATE INDEX singbox_reachability_events_created ON public.singbox_reachability_events USING btree (created_at DESC, id);

CREATE INDEX singbox_reachability_history_age ON public.singbox_reachability_history USING btree (checked_at);

CREATE INDEX singbox_reachability_history_target ON public.singbox_reachability_history USING btree (target_id, id DESC);

CREATE INDEX singbox_reachability_targets_seen ON public.singbox_reachability_targets USING btree (seen_at);

CREATE INDEX singbox_runtime_session_addresses_user ON public.singbox_runtime_session_addresses USING btree (user_id, observed_at);

CREATE INDEX singbox_subscription_source_jobs_due_idx ON public.singbox_subscription_source_jobs USING btree (status, created_at, id);

CREATE INDEX singbox_subscription_source_jobs_history_idx ON public.singbox_subscription_source_jobs USING btree (source_id, created_at DESC, id);

CREATE UNIQUE INDEX singbox_subscription_source_one_active_job_idx ON public.singbox_subscription_source_jobs USING btree (source_id) WHERE (status = ANY (ARRAY['queued'::text, 'running'::text, 'cancelling'::text]));

CREATE INDEX singbox_subscription_source_revisions_history_idx ON public.singbox_subscription_source_revisions USING btree (source_id, generation DESC);

CREATE INDEX singbox_subscription_sources_due_idx ON public.singbox_ordered_subscription_sources USING btree (next_refresh_at, id) WHERE ((deleted_at IS NULL) AND (NOT archived));

CREATE INDEX singbox_usage_daily_day_idx ON public.singbox_usage_daily USING btree (day);

CREATE INDEX singbox_usage_daily_node_idx ON public.singbox_usage_daily USING btree (node_id, day);

CREATE INDEX singbox_user_policies_group_idx ON public.singbox_user_policies USING btree (group_id);

CREATE INDEX system_observer_received ON public.system_observer_reports USING btree (received_at);

CREATE INDEX telemetry_history_age_idx ON public.telemetry_history USING btree (bucket_at, server_id);

CREATE INDEX telemetry_history_compaction_idx ON public.telemetry_history USING btree (resolution_secs, bucket_at, server_id);

CREATE INDEX telemetry_receipts_expiry_idx ON public.telemetry_receipts USING btree (sampled_at);

CREATE INDEX telemetry_samples_expiry_idx ON public.telemetry_samples USING btree (sampled_at);

CREATE INDEX telemetry_samples_time_idx ON public.telemetry_samples USING btree (server_id, sampled_at);

CREATE INDEX usage_records_node_idx ON public.usage_records USING btree (node_id);

CREATE INDEX usage_records_period_idx ON public.usage_records USING btree (period_end);

CREATE INDEX usage_records_user_period_idx ON public.usage_records USING btree (user_id, period_end);

CREATE TRIGGER singbox_endpoint_snapshot_immutable BEFORE DELETE OR UPDATE ON public.singbox_managed_endpoint_versions FOR EACH ROW EXECUTE FUNCTION public.singbox_reject_path_snapshot_change();

CREATE TRIGGER singbox_external_metadata_reference BEFORE INSERT OR UPDATE ON public.singbox_node_metadata FOR EACH ROW EXECUTE FUNCTION public.singbox_check_external_metadata();

CREATE TRIGGER singbox_external_version_immutable BEFORE DELETE OR UPDATE ON public.singbox_ordered_external_node_versions FOR EACH ROW EXECUTE FUNCTION public.singbox_reject_subscription_snapshot_change();

CREATE TRIGGER singbox_nexus_guard_access BEFORE INSERT OR DELETE OR UPDATE ON public.accesses FOR EACH ROW EXECUTE FUNCTION public.singbox_nexus_guard_access();

CREATE TRIGGER singbox_nexus_guard_node BEFORE INSERT OR DELETE OR UPDATE ON public.nodes FOR EACH ROW EXECUTE FUNCTION public.singbox_nexus_guard_node();

CREATE TRIGGER singbox_ordered_path_hop_immutable BEFORE DELETE OR UPDATE ON public.singbox_ordered_chain_hops FOR EACH ROW EXECUTE FUNCTION public.singbox_reject_path_snapshot_change();

CREATE TRIGGER singbox_path_deployment_dependency_immutable BEFORE DELETE OR UPDATE ON public.singbox_path_deployment_dependencies FOR EACH ROW EXECUTE FUNCTION public.singbox_reject_path_snapshot_change();

CREATE TRIGGER singbox_path_runtime_requirement_immutable BEFORE DELETE OR UPDATE ON public.singbox_chain_runtime_requirements FOR EACH ROW EXECUTE FUNCTION public.singbox_reject_path_snapshot_change();

CREATE TRIGGER singbox_path_snapshot_immutable BEFORE DELETE OR UPDATE ON public.singbox_ordered_chain_versions FOR EACH ROW EXECUTE FUNCTION public.singbox_reject_path_snapshot_change();

CREATE TRIGGER singbox_policy_external_reference BEFORE INSERT OR UPDATE ON public.singbox_policy_external_nodes FOR EACH ROW EXECUTE FUNCTION public.singbox_check_policy_external();

CREATE TRIGGER singbox_subscription_membership_immutable BEFORE DELETE OR UPDATE ON public.singbox_subscription_revision_nodes FOR EACH ROW EXECUTE FUNCTION public.singbox_reject_subscription_snapshot_change();

CREATE TRIGGER singbox_subscription_revision_immutable BEFORE DELETE OR UPDATE ON public.singbox_subscription_source_revisions FOR EACH ROW EXECUTE FUNCTION public.singbox_reject_subscription_snapshot_change();

CREATE TRIGGER singbox_usage_daily_insert AFTER INSERT ON public.usage_records REFERENCING NEW TABLE AS inserted FOR EACH STATEMENT EXECUTE FUNCTION public.singbox_usage_daily_apply();

ALTER TABLE ONLY public.accesses
    ADD CONSTRAINT accesses_node_id_fkey FOREIGN KEY (node_id) REFERENCES public.nodes(id);

ALTER TABLE ONLY public.accesses
    ADD CONSTRAINT accesses_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.administrator_passkey_accounts
    ADD CONSTRAINT administrator_passkey_accounts_account_id_fkey FOREIGN KEY (account_id) REFERENCES public.passkey_accounts(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.administrator_passkey_accounts
    ADD CONSTRAINT administrator_passkey_accounts_admin_id_fkey FOREIGN KEY (admin_id) REFERENCES public.admins(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.administrator_preferences
    ADD CONSTRAINT administrator_preferences_admin_id_fkey FOREIGN KEY (admin_id) REFERENCES public.admins(id);

ALTER TABLE ONLY public.administrator_profiles
    ADD CONSTRAINT administrator_profiles_admin_id_fkey FOREIGN KEY (admin_id) REFERENCES public.admins(id);

ALTER TABLE ONLY public.administrator_reauth
    ADD CONSTRAINT administrator_reauth_session_hash_fkey FOREIGN KEY (session_hash) REFERENCES public.sessions(token_hash) ON DELETE CASCADE;

ALTER TABLE ONLY public.administrator_server_grants
    ADD CONSTRAINT administrator_server_grants_admin_id_fkey FOREIGN KEY (admin_id) REFERENCES public.admins(id);

ALTER TABLE ONLY public.administrator_server_grants
    ADD CONSTRAINT administrator_server_grants_server_id_fkey FOREIGN KEY (server_id) REFERENCES public.servers(id);

ALTER TABLE ONLY public.alert_reminder_receipts
    ADD CONSTRAINT alert_reminder_receipts_server_id_fkey FOREIGN KEY (server_id) REFERENCES public.servers(id);

ALTER TABLE ONLY public.alicloud_accounts
    ADD CONSTRAINT alicloud_accounts_credential_fk FOREIGN KEY (credential_id) REFERENCES public.credential_entries(id);

ALTER TABLE ONLY public.alicloud_deliveries
    ADD CONSTRAINT alicloud_deliveries_event_id_fkey FOREIGN KEY (event_id) REFERENCES public.alicloud_events(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.alicloud_eip_rotations
    ADD CONSTRAINT alicloud_eip_rotations_requested_by_fkey FOREIGN KEY (requested_by) REFERENCES public.admins(id);

ALTER TABLE ONLY public.alicloud_eip_rotations
    ADD CONSTRAINT alicloud_eip_rotations_resource_id_fkey FOREIGN KEY (resource_id) REFERENCES public.alicloud_resources(id);

ALTER TABLE ONLY public.alicloud_events
    ADD CONSTRAINT alicloud_events_resource_id_fkey FOREIGN KEY (resource_id) REFERENCES public.alicloud_resources(id);

ALTER TABLE ONLY public.alicloud_managed_security_groups
    ADD CONSTRAINT alicloud_managed_security_groups_operation_id_fkey FOREIGN KEY (operation_id) REFERENCES public.alicloud_security_group_operations(id);

ALTER TABLE ONLY public.alicloud_managed_security_groups
    ADD CONSTRAINT alicloud_managed_security_groups_resource_id_fkey FOREIGN KEY (resource_id) REFERENCES public.alicloud_resources(id);

ALTER TABLE ONLY public.alicloud_operations
    ADD CONSTRAINT alicloud_operations_resource_id_fkey FOREIGN KEY (resource_id) REFERENCES public.alicloud_resources(id);

ALTER TABLE ONLY public.alicloud_power_jobs
    ADD CONSTRAINT alicloud_power_jobs_resource_id_fkey FOREIGN KEY (resource_id) REFERENCES public.alicloud_resources(id);

ALTER TABLE ONLY public.alicloud_resources
    ADD CONSTRAINT alicloud_resources_account_id_fkey FOREIGN KEY (account_id) REFERENCES public.alicloud_accounts(id);

ALTER TABLE ONLY public.alicloud_security_group_operations
    ADD CONSTRAINT alicloud_security_group_operations_requested_by_fkey FOREIGN KEY (requested_by) REFERENCES public.admins(id);

ALTER TABLE ONLY public.alicloud_security_group_operations
    ADD CONSTRAINT alicloud_security_group_operations_resource_id_fkey FOREIGN KEY (resource_id) REFERENCES public.alicloud_resources(id);

ALTER TABLE ONLY public.alicloud_traffic_history
    ADD CONSTRAINT alicloud_traffic_history_account_id_fkey FOREIGN KEY (account_id) REFERENCES public.alicloud_accounts(id);

ALTER TABLE ONLY public.ddns_rules
    ADD CONSTRAINT ddns_rules_server_id_fkey FOREIGN KEY (server_id) REFERENCES public.servers(id);

ALTER TABLE ONLY public.deployments
    ADD CONSTRAINT deployments_server_id_fkey FOREIGN KEY (server_id) REFERENCES public.servers(id);

ALTER TABLE ONLY public.diagnostic_jobs
    ADD CONSTRAINT diagnostic_jobs_server_id_fkey FOREIGN KEY (server_id) REFERENCES public.servers(id);

ALTER TABLE ONLY public.diagnostic_report_sections
    ADD CONSTRAINT diagnostic_report_sections_job_id_fkey FOREIGN KEY (job_id) REFERENCES public.diagnostic_jobs(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.dns_record_history
    ADD CONSTRAINT dns_record_history_requested_by_fkey FOREIGN KEY (requested_by) REFERENCES public.admins(id);

ALTER TABLE ONLY public.enrollment_tokens
    ADD CONSTRAINT enrollment_tokens_server_id_fkey FOREIGN KEY (server_id) REFERENCES public.servers(id);

ALTER TABLE ONLY public.expense_deliveries
    ADD CONSTRAINT expense_deliveries_event_id_fkey FOREIGN KEY (event_id) REFERENCES public.expense_events(id);

ALTER TABLE ONLY public.expense_events
    ADD CONSTRAINT expense_events_subscription_id_fkey FOREIGN KEY (subscription_id) REFERENCES public.expense_subscriptions(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.expense_payments
    ADD CONSTRAINT expense_payments_subscription_id_fkey FOREIGN KEY (subscription_id) REFERENCES public.expense_subscriptions(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.expense_subscriptions
    ADD CONSTRAINT expense_subscriptions_category_id_fkey FOREIGN KEY (category_id) REFERENCES public.expense_labels(id);

ALTER TABLE ONLY public.expense_subscriptions
    ADD CONSTRAINT expense_subscriptions_payer_id_fkey FOREIGN KEY (payer_id) REFERENCES public.expense_labels(id);

ALTER TABLE ONLY public.expense_subscriptions
    ADD CONSTRAINT expense_subscriptions_payment_method_id_fkey FOREIGN KEY (payment_method_id) REFERENCES public.expense_labels(id);

ALTER TABLE ONLY public.expense_subscriptions
    ADD CONSTRAINT expense_subscriptions_replacement_id_fkey FOREIGN KEY (replacement_id) REFERENCES public.expense_subscriptions(id);

ALTER TABLE ONLY public.fleet_config_history
    ADD CONSTRAINT fleet_config_history_operation_id_fkey FOREIGN KEY (operation_id) REFERENCES public.fleet_operations(id);

ALTER TABLE ONLY public.fleet_config_history
    ADD CONSTRAINT fleet_config_history_server_id_fkey FOREIGN KEY (server_id) REFERENCES public.servers(id);

ALTER TABLE ONLY public.fleet_cost_records
    ADD CONSTRAINT fleet_cost_records_server_id_fkey FOREIGN KEY (server_id) REFERENCES public.servers(id);

ALTER TABLE ONLY public.fleet_events
    ADD CONSTRAINT fleet_events_server_id_fkey FOREIGN KEY (server_id) REFERENCES public.servers(id);

ALTER TABLE ONLY public.fleet_operations
    ADD CONSTRAINT fleet_operations_reconciliation_of_fkey FOREIGN KEY (reconciliation_of) REFERENCES public.fleet_operations(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.fleet_operations
    ADD CONSTRAINT fleet_operations_requested_by_fkey FOREIGN KEY (requested_by) REFERENCES public.admins(id);

ALTER TABLE ONLY public.fleet_operations
    ADD CONSTRAINT fleet_operations_server_id_fkey FOREIGN KEY (server_id) REFERENCES public.servers(id);

ALTER TABLE ONLY public.fleet_profiles
    ADD CONSTRAINT fleet_profiles_server_id_fkey FOREIGN KEY (server_id) REFERENCES public.servers(id);

ALTER TABLE ONLY public.fleet_terminal_inputs
    ADD CONSTRAINT fleet_terminal_inputs_session_id_fkey FOREIGN KEY (session_id) REFERENCES public.fleet_terminal_sessions(id);

ALTER TABLE ONLY public.fleet_terminal_outputs
    ADD CONSTRAINT fleet_terminal_outputs_session_id_fkey FOREIGN KEY (session_id) REFERENCES public.fleet_terminal_sessions(id);

ALTER TABLE ONLY public.fleet_terminal_sessions
    ADD CONSTRAINT fleet_terminal_sessions_server_id_fkey FOREIGN KEY (server_id) REFERENCES public.servers(id);

ALTER TABLE ONLY public.management_api_tokens
    ADD CONSTRAINT management_api_tokens_admin_id_fkey FOREIGN KEY (admin_id) REFERENCES public.admins(id);

ALTER TABLE ONLY public.management_audit
    ADD CONSTRAINT management_audit_admin_id_fkey FOREIGN KEY (admin_id) REFERENCES public.admins(id);

ALTER TABLE ONLY public.metrics_minutely
    ADD CONSTRAINT metrics_minutely_server_id_fkey FOREIGN KEY (server_id) REFERENCES public.servers(id);

ALTER TABLE ONLY public.network_acme_jobs
    ADD CONSTRAINT network_acme_jobs_certificate_id_fkey FOREIGN KEY (certificate_id) REFERENCES public.network_documents(id);

ALTER TABLE ONLY public.network_acme_jobs
    ADD CONSTRAINT network_acme_jobs_requested_by_fkey FOREIGN KEY (requested_by) REFERENCES public.admins(id);

ALTER TABLE ONLY public.network_acme_plans
    ADD CONSTRAINT network_acme_plans_certificate_id_fkey FOREIGN KEY (certificate_id) REFERENCES public.network_documents(id);

ALTER TABLE ONLY public.network_acme_plans
    ADD CONSTRAINT network_acme_plans_requested_by_fkey FOREIGN KEY (requested_by) REFERENCES public.admins(id);

ALTER TABLE ONLY public.network_certificate_deployment_previews
    ADD CONSTRAINT network_certificate_deployment_previews_certificate_id_fkey FOREIGN KEY (certificate_id) REFERENCES public.network_documents(id);

ALTER TABLE ONLY public.network_certificate_deployment_previews
    ADD CONSTRAINT network_certificate_deployment_previews_version_id_fkey FOREIGN KEY (version_id) REFERENCES public.network_certificate_versions(id);

ALTER TABLE ONLY public.network_certificate_deployments
    ADD CONSTRAINT network_certificate_deployments_certificate_id_fkey FOREIGN KEY (certificate_id) REFERENCES public.network_documents(id);

ALTER TABLE ONLY public.network_certificate_deployments
    ADD CONSTRAINT network_certificate_deployments_requested_by_fkey FOREIGN KEY (requested_by) REFERENCES public.admins(id);

ALTER TABLE ONLY public.network_certificate_deployments
    ADD CONSTRAINT network_certificate_deployments_server_id_fkey FOREIGN KEY (server_id) REFERENCES public.servers(id);

ALTER TABLE ONLY public.network_certificate_deployments
    ADD CONSTRAINT network_certificate_deployments_version_id_fkey FOREIGN KEY (version_id) REFERENCES public.network_certificate_versions(id);

ALTER TABLE ONLY public.network_certificate_versions
    ADD CONSTRAINT network_certificate_versions_certificate_id_fkey FOREIGN KEY (certificate_id) REFERENCES public.network_documents(id);

ALTER TABLE ONLY public.network_dns_challenges
    ADD CONSTRAINT network_dns_challenges_certificate_id_fkey FOREIGN KEY (certificate_id) REFERENCES public.network_documents(id);

ALTER TABLE ONLY public.network_observations
    ADD CONSTRAINT network_observations_document_id_fkey FOREIGN KEY (document_id) REFERENCES public.network_documents(id);

ALTER TABLE ONLY public.network_observations
    ADD CONSTRAINT network_observations_server_id_fkey FOREIGN KEY (server_id) REFERENCES public.servers(id);

ALTER TABLE ONLY public.network_operation_links
    ADD CONSTRAINT network_operation_links_document_id_fkey FOREIGN KEY (document_id) REFERENCES public.network_documents(id);

ALTER TABLE ONLY public.network_operation_links
    ADD CONSTRAINT network_operation_links_server_id_fkey FOREIGN KEY (server_id) REFERENCES public.servers(id);

ALTER TABLE ONLY public.network_probes
    ADD CONSTRAINT network_probes_server_id_fkey FOREIGN KEY (server_id) REFERENCES public.servers(id);

ALTER TABLE ONLY public.network_probes
    ADD CONSTRAINT network_probes_task_id_fkey FOREIGN KEY (task_id) REFERENCES public.latency_tasks(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.network_server_migration_previews
    ADD CONSTRAINT network_server_migration_previews_requested_by_fkey FOREIGN KEY (requested_by) REFERENCES public.admins(id);

ALTER TABLE ONLY public.network_server_migration_previews
    ADD CONSTRAINT network_server_migration_previews_source_server_id_fkey FOREIGN KEY (source_server_id) REFERENCES public.servers(id);

ALTER TABLE ONLY public.network_server_migration_previews
    ADD CONSTRAINT network_server_migration_previews_target_server_id_fkey FOREIGN KEY (target_server_id) REFERENCES public.servers(id);

ALTER TABLE ONLY public.network_workbench_ip_evidence
    ADD CONSTRAINT network_workbench_ip_evidence_server_id_fkey FOREIGN KEY (server_id) REFERENCES public.servers(id);

ALTER TABLE ONLY public.network_workbench_pairings
    ADD CONSTRAINT network_workbench_pairings_run_id_fkey FOREIGN KEY (run_id) REFERENCES public.network_workbench_runs(id);

ALTER TABLE ONLY public.network_workbench_results
    ADD CONSTRAINT network_workbench_results_job_id_fkey FOREIGN KEY (job_id) REFERENCES public.diagnostic_jobs(id);

ALTER TABLE ONLY public.network_workbench_results
    ADD CONSTRAINT network_workbench_results_run_id_fkey FOREIGN KEY (run_id) REFERENCES public.network_workbench_runs(id);

ALTER TABLE ONLY public.network_workbench_results
    ADD CONSTRAINT network_workbench_results_server_id_fkey FOREIGN KEY (server_id) REFERENCES public.servers(id);

ALTER TABLE ONLY public.network_workbench_runs
    ADD CONSTRAINT network_workbench_runs_plan_id_fkey FOREIGN KEY (plan_id) REFERENCES public.network_workbench_plans(id);

ALTER TABLE ONLY public.network_workbench_shares
    ADD CONSTRAINT network_workbench_shares_run_id_fkey FOREIGN KEY (run_id) REFERENCES public.network_workbench_runs(id);

ALTER TABLE ONLY public.nodes
    ADD CONSTRAINT nodes_server_id_fkey FOREIGN KEY (server_id) REFERENCES public.servers(id);

ALTER TABLE ONLY public.notification_outbox
    ADD CONSTRAINT notification_outbox_event_id_fkey FOREIGN KEY (event_id) REFERENCES public.server_alert_events(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.operations_backup_records
    ADD CONSTRAINT operations_backup_records_imported_by_fkey FOREIGN KEY (imported_by) REFERENCES public.admins(id);

ALTER TABLE ONLY public.operations_backup_records
    ADD CONSTRAINT operations_backup_records_source_schedule_fkey FOREIGN KEY (source_schedule) REFERENCES public.operations_backup_schedules(id);

ALTER TABLE ONLY public.operations_backup_schedules
    ADD CONSTRAINT operations_backup_schedules_requested_by_fkey FOREIGN KEY (requested_by) REFERENCES public.admins(id);

ALTER TABLE ONLY public.operations_cancellation_reminders
    ADD CONSTRAINT operations_cancellation_reminders_configured_by_fkey FOREIGN KEY (configured_by) REFERENCES public.admins(id);

ALTER TABLE ONLY public.operations_cancellation_reminders
    ADD CONSTRAINT operations_cancellation_reminders_incident_id_fkey FOREIGN KEY (incident_id) REFERENCES public.operations_incidents(id);

ALTER TABLE ONLY public.operations_cancellation_reminders
    ADD CONSTRAINT operations_cancellation_reminders_record_id_fkey FOREIGN KEY (record_id) REFERENCES public.fleet_cost_records(id);

ALTER TABLE ONLY public.operations_cloud_links
    ADD CONSTRAINT operations_cloud_links_resource_id_fkey FOREIGN KEY (resource_id) REFERENCES public.alicloud_resources(id);

ALTER TABLE ONLY public.operations_cloud_links
    ADD CONSTRAINT operations_cloud_links_server_id_fkey FOREIGN KEY (server_id) REFERENCES public.servers(id);

ALTER TABLE ONLY public.operations_cloud_links
    ADD CONSTRAINT operations_cloud_links_updated_by_fkey FOREIGN KEY (updated_by) REFERENCES public.admins(id);

ALTER TABLE ONLY public.operations_cloud_observations
    ADD CONSTRAINT operations_cloud_observations_resource_id_fkey FOREIGN KEY (resource_id) REFERENCES public.alicloud_resources(id);

ALTER TABLE ONLY public.operations_hetzner_accounts
    ADD CONSTRAINT operations_hetzner_accounts_updated_by_fkey FOREIGN KEY (updated_by) REFERENCES public.admins(id);

ALTER TABLE ONLY public.operations_hetzner_accounts
    ADD CONSTRAINT operations_hetzner_credential_fk FOREIGN KEY (credential_id) REFERENCES public.credential_entries(id);

ALTER TABLE ONLY public.operations_hetzner_observations
    ADD CONSTRAINT operations_hetzner_observations_refresh_id_fkey FOREIGN KEY (refresh_id) REFERENCES public.operations_hetzner_refreshes(id);

ALTER TABLE ONLY public.operations_hetzner_observations
    ADD CONSTRAINT operations_hetzner_observations_resource_id_fkey FOREIGN KEY (resource_id) REFERENCES public.operations_hetzner_resources(id);

ALTER TABLE ONLY public.operations_hetzner_refreshes
    ADD CONSTRAINT operations_hetzner_refreshes_account_id_fkey FOREIGN KEY (account_id) REFERENCES public.operations_hetzner_accounts(id);

ALTER TABLE ONLY public.operations_hetzner_resources
    ADD CONSTRAINT operations_hetzner_resources_account_id_fkey FOREIGN KEY (account_id) REFERENCES public.operations_hetzner_accounts(id);

ALTER TABLE ONLY public.operations_hetzner_resources
    ADD CONSTRAINT operations_hetzner_resources_server_id_fkey FOREIGN KEY (server_id) REFERENCES public.servers(id);

ALTER TABLE ONLY public.operations_hetzner_resources
    ADD CONSTRAINT operations_hetzner_resources_updated_by_fkey FOREIGN KEY (updated_by) REFERENCES public.admins(id);

ALTER TABLE ONLY public.operations_history
    ADD CONSTRAINT operations_history_actor_fkey FOREIGN KEY (actor) REFERENCES public.admins(id);

ALTER TABLE ONLY public.operations_history
    ADD CONSTRAINT operations_history_job_id_fkey FOREIGN KEY (job_id) REFERENCES public.operations_jobs(id);

ALTER TABLE ONLY public.operations_incident_deliveries
    ADD CONSTRAINT operations_incident_deliveries_incident_id_fkey FOREIGN KEY (incident_id) REFERENCES public.operations_incidents(id);

ALTER TABLE ONLY public.operations_incident_notes
    ADD CONSTRAINT operations_incident_notes_actor_fkey FOREIGN KEY (actor) REFERENCES public.admins(id);

ALTER TABLE ONLY public.operations_incident_notes
    ADD CONSTRAINT operations_incident_notes_incident_id_fkey FOREIGN KEY (incident_id) REFERENCES public.operations_incidents(id);

ALTER TABLE ONLY public.operations_incidents
    ADD CONSTRAINT operations_incidents_acknowledged_by_fkey FOREIGN KEY (acknowledged_by) REFERENCES public.admins(id);

ALTER TABLE ONLY public.operations_incidents
    ADD CONSTRAINT operations_incidents_assignee_fkey FOREIGN KEY (assignee) REFERENCES public.admins(id);

ALTER TABLE ONLY public.operations_incidents
    ADD CONSTRAINT operations_incidents_notification_event_fkey FOREIGN KEY (notification_event) REFERENCES public.server_alert_events(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.operations_incidents
    ADD CONSTRAINT operations_incidents_server_id_fkey FOREIGN KEY (server_id) REFERENCES public.servers(id);

ALTER TABLE ONLY public.operations_jobs
    ADD CONSTRAINT operations_job_schedule_fk FOREIGN KEY (source_schedule) REFERENCES public.operations_schedules(id);

ALTER TABLE ONLY public.operations_jobs
    ADD CONSTRAINT operations_jobs_requested_by_fkey FOREIGN KEY (requested_by) REFERENCES public.admins(id);

ALTER TABLE ONLY public.operations_jobs
    ADD CONSTRAINT operations_jobs_template_id_fkey FOREIGN KEY (template_id) REFERENCES public.operations_templates(id);

ALTER TABLE ONLY public.operations_maintenance
    ADD CONSTRAINT operations_maintenance_created_by_fkey FOREIGN KEY (created_by) REFERENCES public.admins(id);

ALTER TABLE ONLY public.operations_panel_steps
    ADD CONSTRAINT operations_panel_steps_backup_id_fkey FOREIGN KEY (backup_id) REFERENCES public.operations_backup_records(id);

ALTER TABLE ONLY public.operations_panel_steps
    ADD CONSTRAINT operations_panel_steps_job_id_fkey FOREIGN KEY (job_id) REFERENCES public.operations_jobs(id);

ALTER TABLE ONLY public.operations_remediation_rules
    ADD CONSTRAINT operations_remediation_rules_last_job_fkey FOREIGN KEY (last_job) REFERENCES public.operations_jobs(id);

ALTER TABLE ONLY public.operations_remediation_rules
    ADD CONSTRAINT operations_remediation_rules_requested_by_fkey FOREIGN KEY (requested_by) REFERENCES public.admins(id);

ALTER TABLE ONLY public.operations_restore_drills
    ADD CONSTRAINT operations_restore_drills_backup_id_fkey FOREIGN KEY (backup_id) REFERENCES public.operations_backup_records(id);

ALTER TABLE ONLY public.operations_restore_drills
    ADD CONSTRAINT operations_restore_drills_recorded_by_fkey FOREIGN KEY (recorded_by) REFERENCES public.admins(id);

ALTER TABLE ONLY public.operations_schedules
    ADD CONSTRAINT operations_schedules_last_job_fkey FOREIGN KEY (last_job) REFERENCES public.operations_jobs(id);

ALTER TABLE ONLY public.operations_schedules
    ADD CONSTRAINT operations_schedules_requested_by_fkey FOREIGN KEY (requested_by) REFERENCES public.admins(id);

ALTER TABLE ONLY public.operations_server_locks
    ADD CONSTRAINT operations_server_locks_job_id_fkey FOREIGN KEY (job_id) REFERENCES public.operations_jobs(id);

ALTER TABLE ONLY public.operations_server_locks
    ADD CONSTRAINT operations_server_locks_server_id_fkey FOREIGN KEY (server_id) REFERENCES public.servers(id);

ALTER TABLE ONLY public.operations_target_steps
    ADD CONSTRAINT operations_target_steps_fleet_operation_id_fkey FOREIGN KEY (fleet_operation_id) REFERENCES public.fleet_operations(id);

ALTER TABLE ONLY public.operations_target_steps
    ADD CONSTRAINT operations_target_steps_job_id_fkey FOREIGN KEY (job_id) REFERENCES public.operations_jobs(id);

ALTER TABLE ONLY public.operations_target_steps
    ADD CONSTRAINT operations_target_steps_runtime_operation_id_fkey FOREIGN KEY (runtime_operation_id) REFERENCES public.runtime_operations(id);

ALTER TABLE ONLY public.operations_target_steps
    ADD CONSTRAINT operations_target_steps_server_id_fkey FOREIGN KEY (server_id) REFERENCES public.servers(id);

ALTER TABLE ONLY public.operations_templates
    ADD CONSTRAINT operations_templates_created_by_fkey FOREIGN KEY (created_by) REFERENCES public.admins(id);

ALTER TABLE ONLY public.passkey_ceremonies
    ADD CONSTRAINT passkey_ceremonies_account_id_fkey FOREIGN KEY (account_id) REFERENCES public.passkey_accounts(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.passkey_credentials
    ADD CONSTRAINT passkey_credentials_account_id_fkey FOREIGN KEY (account_id) REFERENCES public.passkey_accounts(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.probe_leases
    ADD CONSTRAINT probe_leases_server_id_fkey FOREIGN KEY (server_id) REFERENCES public.servers(id);

ALTER TABLE ONLY public.probe_results
    ADD CONSTRAINT probe_results_server_id_fkey FOREIGN KEY (server_id) REFERENCES public.servers(id);

ALTER TABLE ONLY public.remote_commands
    ADD CONSTRAINT remote_commands_server_id_fkey FOREIGN KEY (server_id) REFERENCES public.servers(id);

ALTER TABLE ONLY public.runtime_apply_facts
    ADD CONSTRAINT runtime_apply_facts_server_id_fkey FOREIGN KEY (server_id) REFERENCES public.servers(id);

ALTER TABLE ONLY public.runtime_apply_facts
    ADD CONSTRAINT runtime_apply_facts_server_id_module_rev_fkey FOREIGN KEY (server_id, module, rev) REFERENCES public.deployments(server_id, module, rev);

ALTER TABLE ONLY public.runtime_control_devices
    ADD CONSTRAINT runtime_control_devices_server_id_fkey FOREIGN KEY (server_id) REFERENCES public.servers(id);

ALTER TABLE ONLY public.runtime_control_receipts
    ADD CONSTRAINT runtime_control_receipts_request_id_fkey FOREIGN KEY (request_id) REFERENCES public.runtime_control_requests(request_id);

ALTER TABLE ONLY public.runtime_control_requests
    ADD CONSTRAINT runtime_control_requests_server_id_fkey FOREIGN KEY (server_id) REFERENCES public.servers(id);

ALTER TABLE ONLY public.runtime_deployment_bindings
    ADD CONSTRAINT runtime_deployment_bindings_server_id_module_rev_fkey FOREIGN KEY (server_id, module, rev) REFERENCES public.deployments(server_id, module, rev);

ALTER TABLE ONLY public.runtime_enablements
    ADD CONSTRAINT runtime_enablements_server_id_fkey FOREIGN KEY (server_id) REFERENCES public.servers(id);

ALTER TABLE ONLY public.runtime_module_checkpoints
    ADD CONSTRAINT runtime_module_checkpoints_barrier_request_id_fkey FOREIGN KEY (barrier_request_id) REFERENCES public.runtime_control_receipts(request_id);

ALTER TABLE ONLY public.runtime_module_checkpoints
    ADD CONSTRAINT runtime_module_checkpoints_checkpoint_request_id_fkey FOREIGN KEY (checkpoint_request_id) REFERENCES public.runtime_control_receipts(request_id);

ALTER TABLE ONLY public.runtime_module_checkpoints
    ADD CONSTRAINT runtime_module_checkpoints_server_id_fkey FOREIGN KEY (server_id) REFERENCES public.servers(id);

ALTER TABLE ONLY public.runtime_operations
    ADD CONSTRAINT runtime_operations_automation_job_id_fkey FOREIGN KEY (automation_job_id) REFERENCES public.operations_jobs(id);

ALTER TABLE ONLY public.runtime_operations
    ADD CONSTRAINT runtime_operations_server_id_fkey FOREIGN KEY (server_id) REFERENCES public.servers(id);

ALTER TABLE ONLY public.runtime_validations
    ADD CONSTRAINT runtime_validations_server_id_fkey FOREIGN KEY (server_id) REFERENCES public.servers(id);

ALTER TABLE ONLY public.server_ip_quality_datasets
    ADD CONSTRAINT server_ip_quality_datasets_server_id_ip_provider_fkey FOREIGN KEY (server_id, ip, provider) REFERENCES public.server_ip_quality(server_id, ip, provider);

ALTER TABLE ONLY public.server_ip_quality_node_generations
    ADD CONSTRAINT server_ip_quality_node_generations_job_id_fkey FOREIGN KEY (job_id) REFERENCES public.diagnostic_jobs(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.server_ip_quality_node_generations
    ADD CONSTRAINT server_ip_quality_node_generations_server_id_fkey FOREIGN KEY (server_id) REFERENCES public.servers(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.server_ip_quality
    ADD CONSTRAINT server_ip_quality_server_id_fkey FOREIGN KEY (server_id) REFERENCES public.servers(id);

ALTER TABLE ONLY public.server_module_status
    ADD CONSTRAINT server_module_status_server_id_fkey FOREIGN KEY (server_id) REFERENCES public.servers(id);

ALTER TABLE ONLY public.server_network_counters
    ADD CONSTRAINT server_network_counters_server_id_fkey FOREIGN KEY (server_id) REFERENCES public.servers(id);

ALTER TABLE ONLY public.server_network_daily
    ADD CONSTRAINT server_network_daily_server_id_fkey FOREIGN KEY (server_id) REFERENCES public.servers(id);

ALTER TABLE ONLY public.server_alert_events
    ADD CONSTRAINT server_offline_events_server_id_fkey FOREIGN KEY (server_id) REFERENCES public.servers(id);

ALTER TABLE ONLY public.server_retirements
    ADD CONSTRAINT server_retirements_server_id_fkey FOREIGN KEY (server_id) REFERENCES public.servers(id);

ALTER TABLE ONLY public.server_traffic_corrections
    ADD CONSTRAINT server_traffic_corrections_server_id_fkey FOREIGN KEY (server_id) REFERENCES public.servers(id);

ALTER TABLE ONLY public.sessions
    ADD CONSTRAINT sessions_admin_id_fkey FOREIGN KEY (admin_id) REFERENCES public.admins(id);

ALTER TABLE ONLY public.singbox_account_challenges
    ADD CONSTRAINT singbox_account_challenges_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.singbox_chain_runtime_requirements
    ADD CONSTRAINT singbox_chain_runtime_requirements_chain_id_generation_fkey FOREIGN KEY (chain_id, generation) REFERENCES public.singbox_ordered_chain_versions(chain_id, generation);

ALTER TABLE ONLY public.singbox_chain_runtime_requirements
    ADD CONSTRAINT singbox_chain_runtime_requirements_server_id_fkey FOREIGN KEY (server_id) REFERENCES public.servers(id);

ALTER TABLE ONLY public.singbox_chain_state
    ADD CONSTRAINT singbox_chain_state_chain_id_fkey FOREIGN KEY (chain_id) REFERENCES public.singbox_chains(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.singbox_chains
    ADD CONSTRAINT singbox_chains_entry_node_id_fkey FOREIGN KEY (entry_node_id) REFERENCES public.nodes(id);

ALTER TABLE ONLY public.singbox_client_templates
    ADD CONSTRAINT singbox_client_templates_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.singbox_credential_rotations
    ADD CONSTRAINT singbox_credential_rotations_server_id_fkey FOREIGN KEY (server_id) REFERENCES public.servers(id);

ALTER TABLE ONLY public.singbox_credential_rotations
    ADD CONSTRAINT singbox_credential_rotations_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.singbox_deployment_preflights
    ADD CONSTRAINT singbox_deployment_preflights_administrator_id_fkey FOREIGN KEY (administrator_id) REFERENCES public.admins(id);

ALTER TABLE ONLY public.singbox_deployment_preflights
    ADD CONSTRAINT singbox_deployment_preflights_confirmed_by_fkey FOREIGN KEY (confirmed_by) REFERENCES public.admins(id);

ALTER TABLE ONLY public.singbox_deployment_preflights
    ADD CONSTRAINT singbox_deployment_preflights_server_id_fkey FOREIGN KEY (server_id) REFERENCES public.servers(id);

ALTER TABLE ONLY public.singbox_deployment_projections
    ADD CONSTRAINT singbox_deployment_projections_server_id_fkey FOREIGN KEY (server_id) REFERENCES public.servers(id);

ALTER TABLE ONLY public.singbox_deployment_public_projection
    ADD CONSTRAINT singbox_deployment_public_projec_server_id_module_revision_fkey FOREIGN KEY (server_id, module, revision) REFERENCES public.deployments(server_id, module, rev);

ALTER TABLE ONLY public.singbox_deployment_public_projection
    ADD CONSTRAINT singbox_deployment_public_projection_node_id_fkey FOREIGN KEY (node_id) REFERENCES public.nodes(id);

ALTER TABLE ONLY public.singbox_entitlement_state
    ADD CONSTRAINT singbox_entitlement_state_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.singbox_installation
    ADD CONSTRAINT singbox_installation_server_id_fkey FOREIGN KEY (server_id) REFERENCES public.servers(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.singbox_invitation_policy_groups
    ADD CONSTRAINT singbox_invitation_policy_groups_group_id_fkey FOREIGN KEY (group_id) REFERENCES public.singbox_policy_groups(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.singbox_invitation_quota_events
    ADD CONSTRAINT singbox_invitation_quota_events_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.singbox_invitation_quotas
    ADD CONSTRAINT singbox_invitation_quotas_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.singbox_invitation_settings
    ADD CONSTRAINT singbox_invitation_settings_package_group_id_fkey FOREIGN KEY (package_group_id) REFERENCES public.singbox_package_groups(id);

ALTER TABLE ONLY public.singbox_invitations
    ADD CONSTRAINT singbox_invitations_issuer_id_fkey FOREIGN KEY (issuer_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.singbox_invitations
    ADD CONSTRAINT singbox_invitations_recipient_id_fkey FOREIGN KEY (recipient_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.singbox_mail_settings
    ADD CONSTRAINT singbox_mail_settings_credential_id_fkey FOREIGN KEY (credential_id) REFERENCES public.credential_entries(id);

ALTER TABLE ONLY public.singbox_managed_endpoint_versions
    ADD CONSTRAINT singbox_managed_endpoint_versions_node_id_fkey FOREIGN KEY (node_id) REFERENCES public.nodes(id);

ALTER TABLE ONLY public.singbox_managed_endpoint_versions
    ADD CONSTRAINT singbox_managed_endpoint_versions_server_id_fkey FOREIGN KEY (server_id) REFERENCES public.servers(id);

ALTER TABLE ONLY public.singbox_member_messages
    ADD CONSTRAINT singbox_member_messages_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.singbox_members
    ADD CONSTRAINT singbox_members_inviter_id_fkey FOREIGN KEY (inviter_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.singbox_members
    ADD CONSTRAINT singbox_members_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.singbox_nexus_bridge
    ADD CONSTRAINT singbox_nexus_bridge_import_id_fkey FOREIGN KEY (import_id) REFERENCES public.singbox_nexus_imports(id);

ALTER TABLE ONLY public.singbox_nexus_device_state
    ADD CONSTRAINT singbox_nexus_device_state_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.singbox_nexus_identities
    ADD CONSTRAINT singbox_nexus_identities_import_id_fkey FOREIGN KEY (import_id) REFERENCES public.singbox_nexus_imports(id);

ALTER TABLE ONLY public.singbox_nexus_identities
    ADD CONSTRAINT singbox_nexus_identities_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.singbox_nexus_metering
    ADD CONSTRAINT singbox_nexus_metering_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.singbox_nexus_nodes
    ADD CONSTRAINT singbox_nexus_nodes_credential_id_fkey FOREIGN KEY (credential_id) REFERENCES public.credential_entries(id);

ALTER TABLE ONLY public.singbox_nexus_nodes
    ADD CONSTRAINT singbox_nexus_nodes_server_id_fkey FOREIGN KEY (server_id) REFERENCES public.servers(id);

ALTER TABLE ONLY public.singbox_nexus_nodes
    ADD CONSTRAINT singbox_nexus_nodes_source_id_fkey FOREIGN KEY (source_id) REFERENCES public.singbox_nexus_node_imports(source_id);

ALTER TABLE ONLY public.singbox_nexus_plan_mappings
    ADD CONSTRAINT singbox_nexus_plan_mappings_import_id_fkey FOREIGN KEY (import_id) REFERENCES public.singbox_nexus_imports(id);

ALTER TABLE ONLY public.singbox_nexus_plan_mappings
    ADD CONSTRAINT singbox_nexus_plan_mappings_package_group_id_fkey FOREIGN KEY (package_group_id) REFERENCES public.singbox_package_groups(id);

ALTER TABLE ONLY public.singbox_nexus_runtime_hosts
    ADD CONSTRAINT singbox_nexus_runtime_hosts_credential_id_fkey FOREIGN KEY (credential_id) REFERENCES public.credential_entries(id);

ALTER TABLE ONLY public.singbox_nexus_runtime_hosts
    ADD CONSTRAINT singbox_nexus_runtime_hosts_server_id_fkey FOREIGN KEY (server_id) REFERENCES public.servers(id);

ALTER TABLE ONLY public.singbox_nexus_runtime_hosts
    ADD CONSTRAINT singbox_nexus_runtime_hosts_source_id_fkey FOREIGN KEY (source_id) REFERENCES public.singbox_nexus_node_imports(source_id);

ALTER TABLE ONLY public.singbox_nexus_runtime_nodes
    ADD CONSTRAINT singbox_nexus_runtime_nodes_inventory_id_fkey FOREIGN KEY (inventory_id) REFERENCES public.singbox_nexus_nodes(id);

ALTER TABLE ONLY public.singbox_nexus_runtime_nodes
    ADD CONSTRAINT singbox_nexus_runtime_nodes_node_id_fkey FOREIGN KEY (node_id) REFERENCES public.nodes(id);

ALTER TABLE ONLY public.singbox_nexus_runtime_nodes
    ADD CONSTRAINT singbox_nexus_runtime_nodes_server_id_fkey FOREIGN KEY (server_id) REFERENCES public.singbox_nexus_runtime_hosts(server_id);

ALTER TABLE ONLY public.singbox_nexus_usage_deliveries
    ADD CONSTRAINT singbox_nexus_usage_deliveries_server_id_epoch_seq_fkey FOREIGN KEY (server_id, epoch, seq) REFERENCES public.usage_batches(server_id, epoch, seq);

ALTER TABLE ONLY public.singbox_nexus_usage_deliveries
    ADD CONSTRAINT singbox_nexus_usage_deliveries_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.singbox_node_metadata
    ADD CONSTRAINT singbox_node_metadata_chain_id_fkey FOREIGN KEY (chain_id) REFERENCES public.singbox_chains(id);

ALTER TABLE ONLY public.singbox_node_metadata
    ADD CONSTRAINT singbox_node_metadata_node_id_fkey FOREIGN KEY (node_id) REFERENCES public.nodes(id);

ALTER TABLE ONLY public.singbox_operation_events
    ADD CONSTRAINT singbox_operation_events_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.singbox_ordered_chain_hops
    ADD CONSTRAINT singbox_ordered_chain_hops_chain_id_generation_fkey FOREIGN KEY (chain_id, generation) REFERENCES public.singbox_ordered_chain_versions(chain_id, generation);

ALTER TABLE ONLY public.singbox_ordered_chain_hops
    ADD CONSTRAINT singbox_ordered_chain_hops_endpoint_version_id_fkey FOREIGN KEY (endpoint_version_id) REFERENCES public.singbox_managed_endpoint_versions(id);

ALTER TABLE ONLY public.singbox_ordered_chain_hops
    ADD CONSTRAINT singbox_ordered_chain_hops_external_node_id_fkey FOREIGN KEY (external_node_id) REFERENCES public.singbox_ordered_external_nodes(id);

ALTER TABLE ONLY public.singbox_ordered_chain_hops
    ADD CONSTRAINT singbox_ordered_chain_hops_managed_node_id_fkey FOREIGN KEY (managed_node_id) REFERENCES public.nodes(id);

ALTER TABLE ONLY public.singbox_ordered_chain_hops
    ADD CONSTRAINT singbox_ordered_chain_hops_managed_server_id_fkey FOREIGN KEY (managed_server_id) REFERENCES public.servers(id);

ALTER TABLE ONLY public.singbox_ordered_chain_hops
    ADD CONSTRAINT singbox_ordered_chain_hops_node_version_id_fkey FOREIGN KEY (node_version_id) REFERENCES public.singbox_ordered_external_node_versions(id);

ALTER TABLE ONLY public.singbox_ordered_chain_hops
    ADD CONSTRAINT singbox_ordered_chain_hops_source_id_fkey FOREIGN KEY (source_id) REFERENCES public.singbox_ordered_subscription_sources(id);

ALTER TABLE ONLY public.singbox_ordered_chain_hops
    ADD CONSTRAINT singbox_ordered_chain_hops_source_revision_id_fkey FOREIGN KEY (source_revision_id) REFERENCES public.singbox_subscription_source_revisions(id);

ALTER TABLE ONLY public.singbox_ordered_chain_versions
    ADD CONSTRAINT singbox_ordered_chain_versions_chain_id_fkey FOREIGN KEY (chain_id) REFERENCES public.singbox_chains(id);

ALTER TABLE ONLY public.singbox_ordered_chain_versions
    ADD CONSTRAINT singbox_ordered_chain_versions_entry_endpoint_version_fkey FOREIGN KEY (entry_endpoint_version) REFERENCES public.singbox_managed_endpoint_versions(id);

ALTER TABLE ONLY public.singbox_ordered_external_node_versions
    ADD CONSTRAINT singbox_ordered_external_node_versions_node_id_fkey FOREIGN KEY (node_id) REFERENCES public.singbox_ordered_external_nodes(id);

ALTER TABLE ONLY public.singbox_ordered_external_node_versions
    ADD CONSTRAINT singbox_ordered_external_node_versions_source_revision_id_fkey FOREIGN KEY (source_revision_id) REFERENCES public.singbox_subscription_source_revisions(id);

ALTER TABLE ONLY public.singbox_ordered_external_nodes
    ADD CONSTRAINT singbox_ordered_external_nodes_last_seen_revision_fkey FOREIGN KEY (last_seen_revision) REFERENCES public.singbox_subscription_source_revisions(id);

ALTER TABLE ONLY public.singbox_ordered_external_nodes
    ADD CONSTRAINT singbox_ordered_external_nodes_latest_version_fkey FOREIGN KEY (latest_version) REFERENCES public.singbox_ordered_external_node_versions(id);

ALTER TABLE ONLY public.singbox_ordered_external_nodes
    ADD CONSTRAINT singbox_ordered_external_nodes_source_id_fkey FOREIGN KEY (source_id) REFERENCES public.singbox_ordered_subscription_sources(id);

ALTER TABLE ONLY public.singbox_ordered_source_previews
    ADD CONSTRAINT singbox_ordered_source_previews_admin_id_fkey FOREIGN KEY (admin_id) REFERENCES public.admins(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.singbox_ordered_subscription_sources
    ADD CONSTRAINT singbox_ordered_subscription_sour_current_success_revision_fkey FOREIGN KEY (current_success_revision) REFERENCES public.singbox_subscription_source_revisions(id);

ALTER TABLE ONLY public.singbox_package_assignments
    ADD CONSTRAINT singbox_package_assignments_package_group_id_fkey FOREIGN KEY (package_group_id) REFERENCES public.singbox_package_groups(id);

ALTER TABLE ONLY public.singbox_package_assignments
    ADD CONSTRAINT singbox_package_assignments_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.singbox_path_controller_secrets
    ADD CONSTRAINT singbox_path_controller_secrets_server_id_fkey FOREIGN KEY (server_id) REFERENCES public.servers(id);

ALTER TABLE ONLY public.singbox_path_deployment_dependencies
    ADD CONSTRAINT singbox_path_deployment_dependen_server_id_module_revision_fkey FOREIGN KEY (server_id, module, revision) REFERENCES public.deployments(server_id, module, rev);

ALTER TABLE ONLY public.singbox_path_deployment_dependencies
    ADD CONSTRAINT singbox_path_deployment_dependencies_chain_id_generation_fkey FOREIGN KEY (chain_id, generation) REFERENCES public.singbox_ordered_chain_versions(chain_id, generation);

ALTER TABLE ONLY public.singbox_path_probes
    ADD CONSTRAINT singbox_path_probes_chain_id_generation_fkey FOREIGN KEY (chain_id, generation) REFERENCES public.singbox_ordered_chain_versions(chain_id, generation);

ALTER TABLE ONLY public.singbox_path_probes
    ADD CONSTRAINT singbox_path_probes_server_id_fkey FOREIGN KEY (server_id) REFERENCES public.servers(id);

ALTER TABLE ONLY public.singbox_path_stage_deployments
    ADD CONSTRAINT singbox_path_stage_deployments_chain_id_generation_fkey FOREIGN KEY (chain_id, generation) REFERENCES public.singbox_ordered_chain_versions(chain_id, generation);

ALTER TABLE ONLY public.singbox_path_stage_deployments
    ADD CONSTRAINT singbox_path_stage_deployments_server_id_fkey FOREIGN KEY (server_id) REFERENCES public.servers(id);

ALTER TABLE ONLY public.singbox_path_stage_deployments
    ADD CONSTRAINT singbox_path_stage_deployments_server_id_module_revision_fkey FOREIGN KEY (server_id, module, revision) REFERENCES public.deployments(server_id, module, rev);

ALTER TABLE ONLY public.singbox_payment_channels
    ADD CONSTRAINT singbox_payment_channels_credential_id_fkey FOREIGN KEY (credential_id) REFERENCES public.credential_entries(id);

ALTER TABLE ONLY public.singbox_policy_chains
    ADD CONSTRAINT singbox_policy_chains_chain_id_fkey FOREIGN KEY (chain_id) REFERENCES public.singbox_chains(id);

ALTER TABLE ONLY public.singbox_policy_chains
    ADD CONSTRAINT singbox_policy_chains_group_id_fkey FOREIGN KEY (group_id) REFERENCES public.singbox_policy_groups(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.singbox_policy_external_nodes
    ADD CONSTRAINT singbox_policy_external_nodes_group_id_fkey FOREIGN KEY (group_id) REFERENCES public.singbox_policy_groups(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.singbox_policy_nodes
    ADD CONSTRAINT singbox_policy_nodes_group_id_fkey FOREIGN KEY (group_id) REFERENCES public.singbox_policy_groups(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.singbox_policy_nodes
    ADD CONSTRAINT singbox_policy_nodes_node_id_fkey FOREIGN KEY (node_id) REFERENCES public.nodes(id);

ALTER TABLE ONLY public.singbox_portal_accounts
    ADD CONSTRAINT singbox_portal_accounts_account_id_fkey FOREIGN KEY (account_id) REFERENCES public.passkey_accounts(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.singbox_portal_accounts
    ADD CONSTRAINT singbox_portal_accounts_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.singbox_portal_sessions
    ADD CONSTRAINT singbox_portal_sessions_account_id_fkey FOREIGN KEY (account_id) REFERENCES public.passkey_accounts(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.singbox_quota_credits
    ADD CONSTRAINT singbox_quota_credits_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.singbox_reachability_deliveries
    ADD CONSTRAINT singbox_reachability_deliveries_event_id_fkey FOREIGN KEY (event_id) REFERENCES public.singbox_reachability_events(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.singbox_reachability_events
    ADD CONSTRAINT singbox_reachability_events_server_id_fkey FOREIGN KEY (server_id) REFERENCES public.servers(id);

ALTER TABLE ONLY public.singbox_reachability_events
    ADD CONSTRAINT singbox_reachability_events_target_id_fkey FOREIGN KEY (target_id) REFERENCES public.singbox_reachability_targets(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.singbox_reachability_history
    ADD CONSTRAINT singbox_reachability_history_target_id_fkey FOREIGN KEY (target_id) REFERENCES public.singbox_reachability_targets(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.singbox_reachability_settings
    ADD CONSTRAINT singbox_reachability_settings_credential_id_fkey FOREIGN KEY (credential_id) REFERENCES public.credential_entries(id);

ALTER TABLE ONLY public.singbox_runtime_manifest_facts
    ADD CONSTRAINT singbox_runtime_manifest_facts_server_id_module_revision_fkey FOREIGN KEY (server_id, module, revision) REFERENCES public.deployments(server_id, module, rev);

ALTER TABLE ONLY public.singbox_runtime_rollout_members
    ADD CONSTRAINT singbox_runtime_rollout_members_rollout_id_fkey FOREIGN KEY (rollout_id) REFERENCES public.singbox_runtime_rollouts(id);

ALTER TABLE ONLY public.singbox_runtime_rollout_members
    ADD CONSTRAINT singbox_runtime_rollout_members_server_id_fkey FOREIGN KEY (server_id) REFERENCES public.servers(id);

ALTER TABLE ONLY public.singbox_runtime_session_addresses
    ADD CONSTRAINT singbox_runtime_session_addresses_server_id_fkey FOREIGN KEY (server_id) REFERENCES public.servers(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.singbox_subscription_domain
    ADD CONSTRAINT singbox_subscription_domain_credential_id_fkey FOREIGN KEY (credential_id) REFERENCES public.credential_entries(id);

ALTER TABLE ONLY public.singbox_subscription_hosts
    ADD CONSTRAINT singbox_subscription_hosts_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.singbox_subscription_revision_nodes
    ADD CONSTRAINT singbox_subscription_revision_nodes_node_id_fkey FOREIGN KEY (node_id) REFERENCES public.singbox_ordered_external_nodes(id);

ALTER TABLE ONLY public.singbox_subscription_revision_nodes
    ADD CONSTRAINT singbox_subscription_revision_nodes_source_revision_id_fkey FOREIGN KEY (source_revision_id) REFERENCES public.singbox_subscription_source_revisions(id);

ALTER TABLE ONLY public.singbox_subscription_revision_nodes
    ADD CONSTRAINT singbox_subscription_revision_nodes_version_id_fkey FOREIGN KEY (version_id) REFERENCES public.singbox_ordered_external_node_versions(id);

ALTER TABLE ONLY public.singbox_subscription_source_jobs
    ADD CONSTRAINT singbox_subscription_source_jobs_source_id_fkey FOREIGN KEY (source_id) REFERENCES public.singbox_ordered_subscription_sources(id);

ALTER TABLE ONLY public.singbox_subscription_source_jobs
    ADD CONSTRAINT singbox_subscription_source_jobs_source_revision_id_fkey FOREIGN KEY (source_revision_id) REFERENCES public.singbox_subscription_source_revisions(id);

ALTER TABLE ONLY public.singbox_subscription_source_revisions
    ADD CONSTRAINT singbox_subscription_source_revisions_job_id_fkey FOREIGN KEY (job_id) REFERENCES public.singbox_subscription_source_jobs(id);

ALTER TABLE ONLY public.singbox_subscription_source_revisions
    ADD CONSTRAINT singbox_subscription_source_revisions_source_id_fkey FOREIGN KEY (source_id) REFERENCES public.singbox_ordered_subscription_sources(id);

ALTER TABLE ONLY public.singbox_usage_daily
    ADD CONSTRAINT singbox_usage_daily_node_id_fkey FOREIGN KEY (node_id) REFERENCES public.nodes(id);

ALTER TABLE ONLY public.singbox_usage_daily
    ADD CONSTRAINT singbox_usage_daily_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.singbox_user_packages
    ADD CONSTRAINT singbox_user_packages_user_id_assignment_id_fkey FOREIGN KEY (user_id, assignment_id) REFERENCES public.singbox_package_assignments(user_id, id);

ALTER TABLE ONLY public.singbox_user_packages
    ADD CONSTRAINT singbox_user_packages_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.singbox_user_policies
    ADD CONSTRAINT singbox_user_policies_group_id_fkey FOREIGN KEY (group_id) REFERENCES public.singbox_policy_groups(id);

ALTER TABLE ONLY public.singbox_user_policies
    ADD CONSTRAINT singbox_user_policies_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.singbox_wallets
    ADD CONSTRAINT singbox_wallets_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.tcpquality_target_regions
    ADD CONSTRAINT tcpquality_target_regions_probe_id_fkey FOREIGN KEY (probe_id) REFERENCES public.network_probes(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.telemetry_history_initialized
    ADD CONSTRAINT telemetry_history_initialized_server_id_fkey FOREIGN KEY (server_id) REFERENCES public.servers(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.telemetry_history
    ADD CONSTRAINT telemetry_history_server_id_fkey FOREIGN KEY (server_id) REFERENCES public.servers(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.telemetry_receipts
    ADD CONSTRAINT telemetry_receipts_server_id_fkey FOREIGN KEY (server_id) REFERENCES public.servers(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.telemetry_samples
    ADD CONSTRAINT telemetry_samples_server_id_fkey FOREIGN KEY (server_id) REFERENCES public.servers(id);

ALTER TABLE ONLY public.usage_batches
    ADD CONSTRAINT usage_batches_server_id_fkey FOREIGN KEY (server_id) REFERENCES public.servers(id);

ALTER TABLE ONLY public.usage_records
    ADD CONSTRAINT usage_records_node_id_fkey FOREIGN KEY (node_id) REFERENCES public.nodes(id);

ALTER TABLE ONLY public.usage_records
    ADD CONSTRAINT usage_records_server_id_epoch_seq_fkey FOREIGN KEY (server_id, epoch, seq) REFERENCES public.usage_batches(server_id, epoch, seq);

ALTER TABLE ONLY public.usage_records
    ADD CONSTRAINT usage_records_server_id_fkey FOREIGN KEY (server_id) REFERENCES public.servers(id);

ALTER TABLE ONLY public.usage_records
    ADD CONSTRAINT usage_records_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id);

-- Initial singleton settings. No historical data transformations.
INSERT INTO public.passkey_accounts VALUES ('00000000-0000-0000-0000-000000000001', 0);
INSERT INTO public.auth_rate_limits VALUES ('global', 0, 0);
INSERT INTO public.exchange_rates VALUES (true, 'CNY', '{"CNY": 1}', '{}', NULL, NULL, NULL, NULL, 0, NULL, 0, NULL);
INSERT INTO public.expense_settings VALUES (true, 'USD', NULL, 'UTC', false, 1);
INSERT INTO public.notification_test_limit VALUES (true, 0);
INSERT INTO public.panel_settings VALUES (true, '{}');
INSERT INTO public.record_retention_policy VALUES ('monitoring', 90, 0);
INSERT INTO public.record_retention_policy VALUES ('logs', 30, 0);
INSERT INTO public.record_retention_policy VALUES ('ip', 30, 0);
INSERT INTO public.record_retention_policy VALUES ('terminal', 7, 0);
INSERT INTO public.record_retention_policy VALUES ('proxy-access', 30, 0);
INSERT INTO public.record_retention_policy VALUES ('audit', 365, 0);
INSERT INTO public.singbox_invitation_settings VALUES (true, false, NULL, 0, 1, 0);
INSERT INTO public.singbox_mail_settings VALUES (true, false, false, '', 465, 'tls', '', '', NULL, '{}', 1);
INSERT INTO public.singbox_operation_privacy VALUES (true, 180, 7, 90);
INSERT INTO public.singbox_reachability_settings VALUES (true, false, 30, NULL, 1, 0, NULL, NULL, '{}');
INSERT INTO public.telemetry_policy VALUES (true, 30);

CREATE TABLE public.singbox_user_limits (
    user_id bigint PRIMARY KEY REFERENCES public.users(id),
    upload_mbps integer NOT NULL DEFAULT 0 CHECK (upload_mbps BETWEEN 0 AND 1000000),
    download_mbps integer NOT NULL DEFAULT 0 CHECK (download_mbps BETWEEN 0 AND 1000000),
    max_connections integer NOT NULL DEFAULT 0 CHECK (max_connections BETWEEN 0 AND 1000000),
    revision bigint NOT NULL DEFAULT 0 CHECK (revision BETWEEN 0 AND 9007199254740991)
);

CREATE TABLE public.singbox_user_controls (
    user_id BIGINT PRIMARY KEY REFERENCES public.users(id),
    paused BOOLEAN NOT NULL DEFAULT FALSE,
    revision BIGINT NOT NULL DEFAULT 1 CHECK(revision>0),
    updated_at BIGINT NOT NULL
);

CREATE TABLE public.singbox_runtime_controls (server_id BIGINT PRIMARY KEY REFERENCES public.servers(id), control_secret TEXT NOT NULL);

CREATE TABLE public.singbox_server_profiles (
    server_id BIGINT PRIMARY KEY REFERENCES public.servers(id),
    definition JSONB NOT NULL,
    revision BIGINT NOT NULL CHECK (revision > 0),
    updated_at BIGINT NOT NULL
);

CREATE TABLE public.device_sessions (
    token_hash text PRIMARY KEY,
    server_id bigint NOT NULL REFERENCES public.servers(id),
    expires_at bigint NOT NULL
);

CREATE INDEX device_sessions_server_idx ON public.device_sessions USING btree (server_id);

CREATE INDEX device_sessions_expiry_idx ON public.device_sessions USING btree (expires_at);

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

CREATE TABLE public.runtime_publications (
    server_id bigint PRIMARY KEY REFERENCES public.servers(id) ON DELETE CASCADE,
    manifest_rev bigint DEFAULT 0 NOT NULL CHECK (manifest_rev >= 0),
    dirty_at bigint
);

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

CREATE TABLE public.cloud_resource_costs (
    resource_id uuid PRIMARY KEY REFERENCES public.alicloud_resources(id),
    purchase_reference text DEFAULT ''::text NOT NULL,
    purchase_amount text,
    currency text,
    monthly_budget text,
    budget_currency text,
    expires_at bigint
);

CREATE TABLE public.apsp_schema_baseline (singleton boolean PRIMARY KEY CHECK (singleton), fingerprint text NOT NULL);
INSERT INTO public.apsp_schema_baseline SELECT true, (SELECT md5(string_agg(definition, E'\n' ORDER BY definition COLLATE "C"))
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
) AS definitions);-- Ordinary and imported users share the same reversible administrative gate.
