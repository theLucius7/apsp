ALTER TABLE singbox_chains ADD COLUMN path_kind TEXT NOT NULL DEFAULT 'legacy'
    CHECK (path_kind IN ('legacy','mixed'));
ALTER TABLE singbox_chains ADD COLUMN active_generation BIGINT;
ALTER TABLE singbox_chains ADD COLUMN pending_generation BIGINT;
ALTER TABLE singbox_chains ADD COLUMN minimum_generation BIGINT NOT NULL DEFAULT 0;
ALTER TABLE singbox_chains ADD COLUMN deleted_at BIGINT;
ALTER TABLE singbox_chains ALTER COLUMN exit_node_id DROP NOT NULL;
ALTER TABLE singbox_chains ALTER COLUMN relay_uuid DROP NOT NULL;
ALTER TABLE singbox_chains ADD CONSTRAINT singbox_chain_projection_kind CHECK (
    (path_kind='legacy' AND exit_node_id IS NOT NULL AND relay_uuid IS NOT NULL)
    OR (path_kind='mixed' AND exit_node_id IS NULL AND relay_uuid IS NULL));
ALTER TABLE singbox_chains DROP CONSTRAINT singbox_chains_entry_node_id_key;
CREATE UNIQUE INDEX singbox_live_chain_entry ON singbox_chains(entry_node_id) WHERE deleted_at IS NULL;
CREATE VIEW singbox_live_chains AS SELECT * FROM singbox_chains WHERE deleted_at IS NULL;

CREATE TABLE singbox_chain_versions (
    chain_id BIGINT NOT NULL REFERENCES singbox_chains(id),
    generation BIGINT NOT NULL CHECK (generation > 0),
    previous_generation BIGINT,
    legacy BOOLEAN NOT NULL DEFAULT FALSE,
    path_json JSONB NOT NULL,
    semantic_hash TEXT NOT NULL,
    networks JSONB NOT NULL,
    stage TEXT NOT NULL CHECK (stage IN ('waiting_dependencies','preparing_entry','checking_candidate',
        'switching_entry','checking_active','establishing_barrier','retiring_old','rolling_back','active','failed','retired')),
    failure_stage TEXT,
    last_error TEXT,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
    PRIMARY KEY(chain_id,generation),
    FOREIGN KEY(chain_id,previous_generation) REFERENCES singbox_chain_versions(chain_id,generation)
);
ALTER TABLE singbox_chains ADD CONSTRAINT singbox_active_generation_fk
    FOREIGN KEY(id,active_generation) REFERENCES singbox_chain_versions(chain_id,generation) DEFERRABLE INITIALLY DEFERRED;
ALTER TABLE singbox_chains ADD CONSTRAINT singbox_pending_generation_fk
    FOREIGN KEY(id,pending_generation) REFERENCES singbox_chain_versions(chain_id,generation) DEFERRABLE INITIALLY DEFERRED;

CREATE TABLE singbox_chain_hops (
    chain_id BIGINT NOT NULL,
    generation BIGINT NOT NULL,
    position INTEGER NOT NULL CHECK (position BETWEEN 0 AND 7),
    kind TEXT NOT NULL CHECK(kind IN ('managed','subscription')),
    managed_node_id BIGINT REFERENCES nodes(id),
    managed_server_id BIGINT REFERENCES servers(id),
    endpoint_json JSONB,
    relay_uuid UUID,
    source_id BIGINT REFERENCES singbox_subscription_sources(id),
    external_node_id BIGINT REFERENCES singbox_external_nodes(id),
    external_version_id BIGINT REFERENCES singbox_external_node_versions(id),
    update_mode TEXT CHECK(update_mode IN ('follow_node','pinned')),
    PRIMARY KEY(chain_id,generation,position),
    FOREIGN KEY(chain_id,generation) REFERENCES singbox_chain_versions(chain_id,generation),
    CHECK ((kind='managed' AND managed_node_id IS NOT NULL AND managed_server_id IS NOT NULL
        AND endpoint_json IS NOT NULL AND relay_uuid IS NOT NULL AND source_id IS NULL
        AND external_node_id IS NULL AND external_version_id IS NULL AND update_mode IS NULL)
      OR (kind='subscription' AND managed_node_id IS NULL AND managed_server_id IS NULL
        AND endpoint_json IS NULL AND relay_uuid IS NULL AND source_id IS NOT NULL
        AND external_node_id IS NOT NULL AND external_version_id IS NOT NULL AND update_mode IS NOT NULL))
);
CREATE INDEX singbox_path_managed_references ON singbox_chain_hops(managed_node_id,chain_id);
CREATE INDEX singbox_path_external_references ON singbox_chain_hops(external_version_id,chain_id);

-- Old relays retain their original native tags and credentials; migration publishes nothing.
INSERT INTO singbox_chain_versions(chain_id,generation,legacy,path_json,semantic_hash,networks,stage,created_at,updated_at)
SELECT c.id,1,TRUE,jsonb_build_object('chain_id',c.id,'generation',1,'entry_server_id',n.server_id,
    'entry_node_id',n.id,'active',TRUE,'hops',jsonb_build_array(jsonb_build_object('kind','managed',
        'server_id',e.server_id,'identity',c.relay_uuid,'endpoint',jsonb_build_object(
            'id',e.id,'name',e.name,'port',e.port,'public_host',e.public_host,'sni',e.sni,
            'private_key',e.private_key,'public_key',e.public_key,'short_id',e.short_id,'users','[]'::jsonb,
            'enabled',e.enabled,'settings',e.settings,'protocol_config',e.protocol_config)))),
    'legacy-preserved','{"tcp":true,"udp":true}'::jsonb,'active',0,0
FROM singbox_chains c JOIN nodes n ON n.id=c.entry_node_id JOIN nodes e ON e.id=c.exit_node_id;
INSERT INTO singbox_chain_hops(chain_id,generation,position,kind,managed_node_id,managed_server_id,endpoint_json,relay_uuid)
SELECT v.chain_id,1,0,'managed',c.exit_node_id,e.server_id,v.path_json->'hops'->0->'endpoint',c.relay_uuid
FROM singbox_chain_versions v JOIN singbox_chains c ON c.id=v.chain_id JOIN nodes e ON e.id=c.exit_node_id;
UPDATE singbox_chains SET active_generation=1;

CREATE FUNCTION singbox_preserve_path_version() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP='DELETE' OR NEW.chain_id<>OLD.chain_id OR NEW.generation<>OLD.generation
        OR NEW.path_json IS DISTINCT FROM OLD.path_json OR NEW.semantic_hash<>OLD.semantic_hash
        OR NEW.networks IS DISTINCT FROM OLD.networks OR NEW.legacy<>OLD.legacy
        OR NEW.previous_generation IS DISTINCT FROM OLD.previous_generation OR NEW.created_at<>OLD.created_at THEN
        RAISE EXCEPTION 'path version contents are immutable';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER singbox_path_version_immutable BEFORE UPDATE OR DELETE ON singbox_chain_versions
    FOR EACH ROW EXECUTE FUNCTION singbox_preserve_path_version();
CREATE FUNCTION singbox_preserve_path_hop() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN RAISE EXCEPTION 'path hop contents are immutable'; END;
$$;
CREATE TRIGGER singbox_path_hop_immutable BEFORE UPDATE OR DELETE ON singbox_chain_hops
    FOR EACH ROW EXECUTE FUNCTION singbox_preserve_path_hop();

CREATE TABLE singbox_chain_creation_requests (
    request_id UUID PRIMARY KEY,
    request_hash TEXT NOT NULL,
    receipt JSONB NOT NULL,
    created_at BIGINT NOT NULL
);
CREATE TABLE singbox_path_controls (
    server_id BIGINT PRIMARY KEY REFERENCES servers(id),
    secret TEXT NOT NULL,
    test_url TEXT NOT NULL
);
CREATE TABLE singbox_deployment_projections (
    server_id BIGINT NOT NULL REFERENCES servers(id),
    rev BIGINT NOT NULL,
    source_json JSONB NOT NULL,
    PRIMARY KEY(server_id,rev)
);
CREATE TABLE singbox_path_deployments (
    chain_id BIGINT NOT NULL,
    generation BIGINT NOT NULL,
    server_id BIGINT NOT NULL REFERENCES servers(id),
    rev BIGINT NOT NULL,
    bundle_sha256 TEXT NOT NULL,
    role TEXT NOT NULL CHECK(role IN ('dependency','candidate','active','retired')),
    PRIMARY KEY(chain_id,generation,server_id,rev,role),
    FOREIGN KEY(chain_id,generation) REFERENCES singbox_chain_versions(chain_id,generation)
);

CREATE FUNCTION singbox_path_resources_available(chain BIGINT) RETURNS BOOLEAN LANGUAGE sql STABLE AS $$
    SELECT EXISTS(SELECT 1 FROM singbox_live_chains c JOIN nodes n ON n.id=c.entry_node_id
        JOIN servers s ON s.id=n.server_id
        WHERE c.id=chain AND n.deleted_at IS NULL AND n.enabled AND n.protocol='vless-reality' AND s.deleted_at IS NULL
        AND ((c.path_kind='legacy' AND EXISTS(SELECT 1 FROM nodes e JOIN servers es ON es.id=e.server_id WHERE e.id=c.exit_node_id AND e.enabled AND e.deleted_at IS NULL AND e.protocol='vless-reality' AND es.deleted_at IS NULL))
          OR (c.path_kind='mixed' AND COALESCE(c.active_generation,c.pending_generation) IS NOT NULL))
        AND NOT EXISTS(SELECT 1 FROM singbox_chain_hops h
            LEFT JOIN nodes m ON m.id=h.managed_node_id LEFT JOIN servers ms ON ms.id=h.managed_server_id
            WHERE h.chain_id=c.id AND h.generation=COALESCE(c.active_generation,c.pending_generation)
            AND h.kind='managed' AND (m.deleted_at IS NOT NULL OR NOT m.enabled OR m.protocol<>'vless-reality' OR ms.deleted_at IS NOT NULL)));
$$;

CREATE OR REPLACE VIEW singbox_desired_accesses AS
    SELECT DISTINCT granted.user_id,granted.node_id FROM (
        SELECT user_id,node_id FROM accesses WHERE direct_grant
        UNION SELECT u.user_id,n.node_id FROM singbox_user_policies u JOIN singbox_policy_nodes n ON n.group_id=u.group_id
        UNION SELECT u.user_id,c.entry_node_id FROM singbox_user_policies u
            JOIN singbox_policy_chains p ON p.group_id=u.group_id JOIN singbox_live_chains c ON c.id=p.chain_id
    ) granted
    JOIN users u ON u.id=granted.user_id AND u.deleted_at IS NULL
    JOIN nodes n ON n.id=granted.node_id AND n.deleted_at IS NULL
    JOIN servers s ON s.id=n.server_id AND s.deleted_at IS NULL
    LEFT JOIN singbox_live_chains c ON c.entry_node_id=n.id
    WHERE c.id IS NULL OR (n.protocol='vless-reality' AND (
        (c.path_kind='legacy' AND EXISTS(SELECT 1 FROM nodes e JOIN servers es ON es.id=e.server_id
            WHERE e.id=c.exit_node_id AND e.protocol='vless-reality' AND e.deleted_at IS NULL AND es.deleted_at IS NULL))
        OR (c.path_kind='mixed' AND singbox_path_resources_available(c.id))));

CREATE OR REPLACE FUNCTION singbox_eligible_accesses(at_s BIGINT)
RETURNS TABLE(user_id BIGINT,node_id BIGINT,uuid UUID,stat_name TEXT,credential TEXT)
LANGUAGE sql STABLE AS $$
    SELECT a.user_id,a.node_id,a.uuid,a.stat_name,a.credential FROM accesses a
    JOIN singbox_desired_accesses d USING(user_id,node_id)
    JOIN singbox_entitlements(at_s) e ON e.user_id=a.user_id AND e.allowed
    JOIN nodes n ON n.id=a.node_id AND n.enabled
    WHERE NOT EXISTS(SELECT 1 FROM singbox_live_chains c JOIN nodes x ON x.id=c.exit_node_id
        WHERE c.entry_node_id=n.id AND NOT x.enabled);
$$;

CREATE FUNCTION singbox_path_ready(chain BIGINT) RETURNS BOOLEAN LANGUAGE sql STABLE AS $$
    SELECT EXISTS(SELECT 1 FROM singbox_live_chains c JOIN nodes n ON n.id=c.entry_node_id
        WHERE c.id=chain AND c.path_kind='mixed' AND singbox_path_resources_available(c.id)
        AND c.active_generation IS NOT NULL AND c.minimum_generation>=c.active_generation
        AND NOT EXISTS(
            SELECT 1 FROM (SELECT n.server_id AS server_id,'active'::text AS role
                UNION SELECT h.managed_server_id,'dependency' FROM singbox_chain_hops h
                WHERE h.chain_id=c.id AND h.generation=c.active_generation AND h.managed_server_id IS NOT NULL) required
            WHERE NOT EXISTS(SELECT 1 FROM server_module_status m JOIN servers s ON s.id=m.server_id
                JOIN singbox_path_deployments d ON d.server_id=m.server_id AND d.rev=m.applied_rev
                JOIN deployments b ON b.server_id=d.server_id AND b.module='singbox' AND b.rev=d.rev AND b.bundle_sha256=d.bundle_sha256
                WHERE m.server_id=required.server_id AND m.module='singbox' AND m.healthy AND m.applied_rev=m.target_rev
                AND s.dirty_at IS NULL AND s.deleted_at IS NULL AND d.chain_id=c.id
                AND d.generation=c.active_generation AND d.role=required.role)));
$$;
