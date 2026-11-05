-- The original Nexus permission groups become policy groups (ADR 0115). Each
-- group keeps the migrated lines it served and the members whose package mapped
-- to it, so nobody gains or loses a line. Migrated lines keep serving the
-- members' original client identities: they never get rows in `accesses`.

ALTER TABLE singbox_policy_groups
    ADD COLUMN legacy_group_id BIGINT UNIQUE CHECK (legacy_group_id > 0);

-- The policy group standing for an original permission group, created on first
-- use and named after the group as the old panel showed it.
CREATE FUNCTION singbox_legacy_policy_group(legacy BIGINT) RETURNS BIGINT
LANGUAGE plpgsql AS $$
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

-- One group for every original group a package maps to or a migrated line names,
-- created in the order of the original ids.
DO $$
DECLARE
    item RECORD;
BEGIN
    FOR item IN
        SELECT legacy_group_id AS legacy FROM singbox_nexus_plan_mappings WHERE legacy_group_id IS NOT NULL
        UNION SELECT unnest(legacy_group_ids) FROM singbox_nexus_runtime_nodes
        ORDER BY legacy
    LOOP
        PERFORM singbox_legacy_policy_group(item.legacy);
    END LOOP;
END $$;

INSERT INTO singbox_policy_nodes(group_id,node_id)
    SELECT g.id,r.node_id FROM singbox_nexus_runtime_nodes r
    JOIN singbox_policy_groups g ON g.legacy_group_id=ANY(r.legacy_group_ids)
    ON CONFLICT DO NOTHING;

-- Members are the users whose current package mapped to the group, whether or
-- not that package is usable right now: renewing it restores the same lines.
INSERT INTO singbox_user_policies(user_id,group_id)
    SELECT i.user_id,g.id FROM singbox_nexus_identities i
    JOIN users u ON u.id=i.user_id AND u.deleted_at IS NULL
    JOIN singbox_user_packages up ON up.user_id=i.user_id
    JOIN singbox_package_assignments a ON a.id=up.assignment_id
    JOIN singbox_nexus_plan_mappings p ON p.package_group_id=a.package_group_id AND p.import_id=i.import_id
    JOIN singbox_policy_groups g ON g.legacy_group_id=p.legacy_group_id
    ON CONFLICT DO NOTHING;

-- Invited accounts used to inherit the lines of their package's original group.
INSERT INTO singbox_invitation_policy_groups(group_id)
    SELECT g.id FROM singbox_invitation_settings s
    JOIN singbox_nexus_plan_mappings p ON p.package_group_id=s.package_group_id
    JOIN singbox_nexus_bridge b ON b.import_id=p.import_id AND b.enabled
    JOIN singbox_policy_groups g ON g.legacy_group_id=p.legacy_group_id
    ON CONFLICT DO NOTHING;

-- The one original group the old panel renders for a member, which holds a single
-- group per user: the group of their package while they are still in it,
-- otherwise their lowest original group, or none.
CREATE FUNCTION singbox_member_legacy_group(member BIGINT) RETURNS BIGINT
LANGUAGE sql STABLE AS $$
    SELECT g.legacy_group_id FROM singbox_user_policies up
    JOIN singbox_policy_groups g ON g.id=up.group_id AND g.legacy_group_id IS NOT NULL
    LEFT JOIN singbox_user_packages pk ON pk.user_id=up.user_id
    LEFT JOIN singbox_package_assignments a ON a.id=pk.assignment_id
    LEFT JOIN singbox_nexus_plan_mappings p ON p.package_group_id=a.package_group_id
    WHERE up.user_id=member
    ORDER BY g.legacy_group_id IS NOT DISTINCT FROM p.legacy_group_id DESC, g.legacy_group_id
    LIMIT 1
$$;

-- Migrated lines are served with the original client identities by the runtime
-- publisher, so policy groups must not mint new credentials for them.
CREATE OR REPLACE VIEW singbox_desired_accesses AS
    SELECT DISTINCT granted.user_id,granted.node_id FROM (
        SELECT u.user_id,n.node_id FROM singbox_user_policies u JOIN singbox_policy_nodes n ON n.group_id=u.group_id WHERE NOT EXISTS(
            SELECT 1 FROM singbox_chains c WHERE c.path_kind='ordered' AND c.entry_node_id=n.node_id AND (c.deleted_at IS NULL OR c.phase<>'retired'))
            AND NOT EXISTS(SELECT 1 FROM singbox_nexus_runtime_nodes r WHERE r.node_id=n.node_id)
        UNION SELECT u.user_id,c.entry_node_id FROM singbox_user_policies u
            JOIN singbox_policy_chains p ON p.group_id=u.group_id JOIN singbox_chains c ON c.id=p.chain_id WHERE c.deleted_at IS NULL
    ) granted
    JOIN users u ON u.id=granted.user_id AND u.deleted_at IS NULL
    JOIN nodes n ON n.id=granted.node_id AND n.deleted_at IS NULL
    JOIN servers s ON s.id=n.server_id AND s.deleted_at IS NULL
    LEFT JOIN singbox_chains c ON c.entry_node_id=n.id AND (c.deleted_at IS NULL OR (c.path_kind='ordered' AND c.phase<>'retired'))
    WHERE c.id IS NULL OR (c.deleted_at IS NULL AND n.protocol='vless-reality' AND (
        (c.path_kind='legacy' AND EXISTS(SELECT 1 FROM nodes e JOIN servers es ON es.id=e.server_id WHERE e.id=c.exit_node_id AND e.protocol='vless-reality' AND e.deleted_at IS NULL AND es.deleted_at IS NULL))
        OR (c.path_kind='mixed' AND singbox_path_resources_available(c.id))
        OR (c.path_kind='ordered' AND EXISTS(SELECT 1 FROM singbox_ordered_chain_versions v WHERE v.chain_id=c.id AND v.generation=c.desired_generation)
            AND NOT EXISTS(SELECT 1 FROM singbox_ordered_chain_hops h LEFT JOIN nodes e ON e.id=h.managed_node_id LEFT JOIN servers es ON es.id=h.managed_server_id
                WHERE h.chain_id=c.id AND h.generation=c.desired_generation AND h.kind='managed' AND (e.id IS NULL OR es.id IS NULL OR e.deleted_at IS NOT NULL OR es.deleted_at IS NOT NULL OR NOT e.enabled)))
    ));

CREATE OR REPLACE FUNCTION singbox_nexus_guard_access() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF EXISTS(SELECT 1 FROM singbox_nexus_runtime_nodes WHERE node_id IN (NEW.node_id,OLD.node_id)) THEN
        RAISE EXCEPTION 'Nexus runtime lines keep the original client identities; grant them through policy groups';
    END IF;
    IF TG_OP='DELETE' THEN RETURN OLD; ELSE RETURN NEW; END IF;
END $$;

-- The old panel follows the member's original group from policy groups now, not
-- from the package. Everything else in the exchange is unchanged.
CREATE OR REPLACE FUNCTION singbox_nexus_bridge_exchange(sample JSONB) RETURNS JSONB
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
END $$;
