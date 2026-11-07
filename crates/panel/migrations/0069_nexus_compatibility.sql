CREATE TABLE singbox_nexus_plan_mappings (
    import_id UUID NOT NULL REFERENCES singbox_nexus_imports(id),
    package_group_id BIGINT PRIMARY KEY REFERENCES singbox_package_groups(id),
    legacy_plan_id BIGINT NOT NULL,
    legacy_group_id BIGINT
);
CREATE TABLE singbox_nexus_bridge (
    singleton BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK(singleton),
    import_id UUID NOT NULL REFERENCES singbox_nexus_imports(id),
    subscription_base TEXT NOT NULL,
    enabled BOOLEAN NOT NULL DEFAULT FALSE,
    pending_generation UUID,
    last_sync_at BIGINT,
    activated_at BIGINT NOT NULL
);
CREATE TABLE singbox_nexus_metering (
    user_id BIGINT PRIMARY KEY REFERENCES users(id),
    upload NUMERIC(30,0) NOT NULL CHECK(upload>=0),
    download NUMERIC(30,0) NOT NULL CHECK(download>=0),
    last_reset_at BIGINT,
    next_reset_at BIGINT,
    observed_at BIGINT NOT NULL
);

-- This operation is available only through the local database role, never HTTP.
CREATE FUNCTION singbox_nexus_bridge_exchange(sample JSONB) RETURNS JSONB
LANGUAGE plpgsql AS $$
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
        'plan_id',p.legacy_plan_id,'group_id',p.legacy_group_id,
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
END;
$$;

-- Imported members keep the legacy meter until their nodes are explicitly adopted.
CREATE OR REPLACE FUNCTION singbox_entitlements(at_s BIGINT)
RETURNS TABLE (user_id BIGINT, package_group_id BIGINT, package_name TEXT, monthly_bytes TEXT,
    reset_day INTEGER, reset_hour INTEGER, reset_minute INTEGER, timezone TEXT,
    starts_at BIGINT, expires_at BIGINT, cycle_start BIGINT, next_reset BIGINT,
    used_bytes TEXT, status TEXT, allowed BOOLEAN)
LANGUAGE sql STABLE AS $$
    WITH evaluated AS (
        SELECT u.id,a.package_group_id,a.package_name,a.monthly_bytes,a.reset_day,a.reset_hour,a.reset_minute,a.timezone,a.starts_at,a.expires_at,
            b.cycle_start,b.next_reset,GREATEST(COALESCE(t.used,0)-COALESCE(c.credit,0),0) AS used,
            CASE WHEN m.state IS NOT NULL AND (m.state<>'active' OR (m.sponsor_required AND m.first_purchase_at IS NULL)) THEN CASE WHEN m.state='active' THEN 'pending' ELSE m.state END
                WHEN a.id IS NULL AND bridge.enabled THEN 'no_package' WHEN a.id IS NULL THEN 'unmetered' WHEN at_s<a.starts_at THEN 'not_started' WHEN at_s>=a.expires_at THEN 'expired'
                WHEN a.monthly_bytes IS NOT NULL AND GREATEST(COALESCE(t.used,0)-COALESCE(c.credit,0),0)>=a.monthly_bytes THEN 'exhausted' ELSE 'active' END AS status
        FROM users u LEFT JOIN singbox_members m ON m.user_id=u.id LEFT JOIN singbox_user_packages p ON p.user_id=u.id
        LEFT JOIN singbox_package_assignments a ON a.id=p.assignment_id
        LEFT JOIN singbox_nexus_identities identity ON identity.user_id=u.id
        LEFT JOIN singbox_nexus_bridge bridge ON bridge.import_id=identity.import_id AND bridge.enabled
        LEFT JOIN singbox_nexus_metering meter ON meter.user_id=u.id AND bridge.singleton
        LEFT JOIN LATERAL singbox_cycle_bounds(at_s,a.reset_day,a.reset_hour,a.reset_minute,a.timezone) native ON TRUE
        LEFT JOIN LATERAL (SELECT CASE WHEN meter.user_id IS NOT NULL THEN COALESCE(meter.last_reset_at,0) ELSE native.cycle_start END AS cycle_start,CASE WHEN meter.user_id IS NOT NULL THEN COALESCE(meter.next_reset_at,253402300799) ELSE native.next_reset END AS next_reset) b ON TRUE
        LEFT JOIN LATERAL (SELECT CASE WHEN meter.user_id IS NOT NULL THEN meter.upload+meter.download ELSE singbox_usage_window(u.id,b.cycle_start,b.next_reset) END AS used) t ON TRUE
        LEFT JOIN LATERAL (SELECT SUM(q.bytes) AS credit FROM singbox_quota_credits q WHERE q.user_id=u.id AND q.cycle_start=b.cycle_start AND q.next_reset=b.next_reset) c ON TRUE
        WHERE u.deleted_at IS NULL
    )
    SELECT id,package_group_id,package_name,monthly_bytes::TEXT,reset_day,reset_hour,reset_minute,timezone,starts_at,NULLIF(expires_at,253402300799),cycle_start,next_reset,used::TEXT,status,status IN('unmetered','active') FROM evaluated;
$$;
