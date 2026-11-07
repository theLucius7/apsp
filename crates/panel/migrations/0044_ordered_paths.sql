-- Versioned paths belong to the proxy plugin; historical deployments and usage stay intact.
ALTER TABLE nodes ADD COLUMN resource_revision BIGINT NOT NULL DEFAULT 1 CHECK(resource_revision>0);
ALTER TABLE singbox_chains ALTER COLUMN exit_node_id DROP NOT NULL;
ALTER TABLE singbox_chains ALTER COLUMN relay_uuid DROP NOT NULL;
ALTER TABLE singbox_chains DROP CONSTRAINT singbox_chains_path_kind_check;
ALTER TABLE singbox_chains ADD CONSTRAINT singbox_chains_path_kind_check CHECK(path_kind IN ('legacy','mixed','ordered'));
ALTER TABLE singbox_chains ADD COLUMN settings_revision BIGINT NOT NULL DEFAULT 1 CHECK(settings_revision>0);
ALTER TABLE singbox_chains ADD COLUMN desired_generation BIGINT NOT NULL DEFAULT 1 CHECK(desired_generation>0);
ALTER TABLE singbox_chains ADD COLUMN applied_generation BIGINT;
ALTER TABLE singbox_chains ADD COLUMN candidate_generation BIGINT;
ALTER TABLE singbox_chains ADD COLUMN recovery_generation BIGINT;
ALTER TABLE singbox_chains ADD CONSTRAINT singbox_ordered_minimum_generation_check CHECK(minimum_generation>=0);
ALTER TABLE singbox_chains ADD COLUMN phase TEXT NOT NULL DEFAULT 'legacy' CHECK(phase IN ('legacy','preparing_dependencies','preparing_entry','probing_candidate','switching_entry','probing_switched','fixing_barrier','retiring_old','applied','restoring','failed','retiring','retired'));
ALTER TABLE singbox_chains ADD COLUMN last_error TEXT;
ALTER TABLE singbox_chains ADD COLUMN route_enabled BOOLEAN NOT NULL DEFAULT TRUE;
ALTER TABLE singbox_chains ADD COLUMN last_granted BOOLEAN NOT NULL DEFAULT FALSE;
ALTER TABLE singbox_chains ADD COLUMN restore_step TEXT CHECK(restore_step IN ('entry','probe','barrier','cleanup'));
-- Existing main deletion history is preserved.
-- The old uniqueness constraint was migrated in 0027; preserve its live index.
CREATE UNIQUE INDEX singbox_chain_one_live_entry ON singbox_chains(entry_node_id) WHERE deleted_at IS NULL OR (path_kind='ordered' AND phase<>'retired');
ALTER TABLE singbox_chains DROP CONSTRAINT singbox_chain_projection_kind;
ALTER TABLE singbox_chains ADD CONSTRAINT singbox_chain_projection_kind CHECK((path_kind='legacy' AND exit_node_id IS NOT NULL AND relay_uuid IS NOT NULL) OR (path_kind IN ('mixed','ordered') AND exit_node_id IS NULL AND relay_uuid IS NULL));
UPDATE singbox_chains SET applied_generation=1 WHERE path_kind='legacy';
CREATE OR REPLACE VIEW singbox_live_chains AS SELECT * FROM singbox_chains WHERE deleted_at IS NULL AND path_kind IN ('legacy','mixed');

CREATE TABLE singbox_managed_endpoint_versions (
    id UUID PRIMARY KEY,
    node_id BIGINT NOT NULL REFERENCES nodes(id),
    server_id BIGINT NOT NULL REFERENCES servers(id),
    snapshot JSONB NOT NULL CHECK(jsonb_typeof(snapshot)='object'),
    semantic_sha256 TEXT NOT NULL CHECK(semantic_sha256 ~ '^[0-9a-f]{64}$'),
    created_at BIGINT NOT NULL,
    UNIQUE(node_id,semantic_sha256)
);
CREATE TABLE singbox_ordered_chain_versions (
    chain_id BIGINT NOT NULL REFERENCES singbox_chains(id),
    generation BIGINT NOT NULL CHECK(generation>0),
    legacy BOOLEAN NOT NULL DEFAULT FALSE,
    entry_endpoint_version UUID NOT NULL REFERENCES singbox_managed_endpoint_versions(id),
    semantic_sha256 TEXT NOT NULL CHECK(semantic_sha256 ~ '^[0-9a-f]{64}$'),
    capabilities JSONB NOT NULL CHECK(jsonb_typeof(capabilities)='object'),
    snapshot JSONB NOT NULL CHECK(jsonb_typeof(snapshot)='object'),
    created_at BIGINT NOT NULL,
    PRIMARY KEY(chain_id,generation)
);
CREATE TABLE singbox_ordered_chain_hops (
    chain_id BIGINT NOT NULL,
    generation BIGINT NOT NULL,
    position INTEGER NOT NULL CHECK(position BETWEEN 1 AND 8),
    kind TEXT NOT NULL CHECK(kind IN ('managed','subscription')),
    endpoint_version_id UUID REFERENCES singbox_managed_endpoint_versions(id),
    managed_node_id BIGINT REFERENCES nodes(id),
    managed_server_id BIGINT REFERENCES servers(id),
    relay_uuid UUID UNIQUE,
    source_id BIGINT REFERENCES singbox_ordered_subscription_sources(id),
    identity_epoch BIGINT,
    external_node_id UUID REFERENCES singbox_ordered_external_nodes(id),
    node_version_id UUID REFERENCES singbox_ordered_external_node_versions(id),
    source_revision_id UUID REFERENCES singbox_subscription_source_revisions(id),
    update_mode TEXT CHECK(update_mode IN ('follow_node','pinned')),
    PRIMARY KEY(chain_id,generation,position),
    FOREIGN KEY(chain_id,generation) REFERENCES singbox_ordered_chain_versions(chain_id,generation),
    CHECK((kind='managed' AND endpoint_version_id IS NOT NULL AND managed_node_id IS NOT NULL AND managed_server_id IS NOT NULL AND relay_uuid IS NOT NULL AND source_id IS NULL AND identity_epoch IS NULL AND external_node_id IS NULL AND node_version_id IS NULL AND source_revision_id IS NULL AND update_mode IS NULL) OR (kind='subscription' AND endpoint_version_id IS NULL AND managed_node_id IS NULL AND managed_server_id IS NULL AND relay_uuid IS NULL AND source_id IS NOT NULL AND identity_epoch>0 AND external_node_id IS NOT NULL AND node_version_id IS NOT NULL AND source_revision_id IS NOT NULL AND update_mode IS NOT NULL))
);
CREATE INDEX singbox_chain_managed_refs ON singbox_ordered_chain_hops(managed_node_id,chain_id,generation);
CREATE INDEX singbox_chain_source_refs ON singbox_ordered_chain_hops(source_id,chain_id,generation);
CREATE INDEX singbox_chain_external_refs ON singbox_ordered_chain_hops(external_node_id,chain_id,generation);
CREATE TABLE singbox_chain_runtime_requirements (
    chain_id BIGINT NOT NULL,generation BIGINT NOT NULL,server_id BIGINT NOT NULL REFERENCES servers(id),
    runtime_version TEXT NOT NULL CHECK(runtime_version='1.14.2'),
    artifact_sha256 TEXT NOT NULL CHECK(artifact_sha256 ~ '^[0-9a-f]{64}$'),artifact JSONB NOT NULL,
    PRIMARY KEY(chain_id,generation,server_id),
    FOREIGN KEY(chain_id,generation) REFERENCES singbox_ordered_chain_versions(chain_id,generation)
);

CREATE TABLE singbox_path_deployment_dependencies (
    server_id BIGINT NOT NULL,
    module TEXT NOT NULL DEFAULT 'singbox',
    revision BIGINT NOT NULL,
    chain_id BIGINT NOT NULL,
    generation BIGINT NOT NULL,
    role TEXT NOT NULL CHECK(role IN ('entry','managed')),
    hop_position INTEGER NOT NULL CHECK(hop_position BETWEEN 0 AND 8),
    route_active BOOLEAN NOT NULL,
    PRIMARY KEY(server_id,module,revision,chain_id,generation,hop_position),
    FOREIGN KEY(server_id,module,revision) REFERENCES deployments(server_id,module,rev),
    FOREIGN KEY(chain_id,generation) REFERENCES singbox_ordered_chain_versions(chain_id,generation)
);
CREATE INDEX singbox_path_dependency_lookup ON singbox_path_deployment_dependencies(chain_id,generation,server_id);
CREATE TABLE singbox_path_stage_deployments (
    chain_id BIGINT NOT NULL,
    generation BIGINT NOT NULL,
    stage TEXT NOT NULL,
    server_id BIGINT NOT NULL REFERENCES servers(id),
    role TEXT NOT NULL CHECK(role IN ('entry','managed')),
    hop_position INTEGER NOT NULL CHECK(hop_position BETWEEN 0 AND 8),
    revision BIGINT NOT NULL,
    bundle_sha256 TEXT NOT NULL CHECK(bundle_sha256 ~ '^[0-9a-f]{64}$'),
    deployment_id UUID NOT NULL,
    binding_digest TEXT NOT NULL CHECK(binding_digest ~ '^[0-9a-f]{64}$'),
    checkpoint_request_id UUID,
    barrier_request_id UUID,
    barrier_vector JSONB,
    observed_at BIGINT,
    PRIMARY KEY(chain_id,generation,stage,server_id),
    FOREIGN KEY(chain_id,generation) REFERENCES singbox_ordered_chain_versions(chain_id,generation)
);
-- The deployment foreign key is to immutable evidence, not the server's moving manifest revision.
ALTER TABLE singbox_path_stage_deployments ADD COLUMN module TEXT NOT NULL DEFAULT 'singbox';
ALTER TABLE singbox_path_stage_deployments ADD FOREIGN KEY(server_id,module,revision) REFERENCES deployments(server_id,module,rev);
CREATE TABLE singbox_path_probes (
    probe_id UUID PRIMARY KEY,
    chain_id BIGINT NOT NULL,
    generation BIGINT NOT NULL,
    stage TEXT NOT NULL CHECK(stage IN ('candidate','switched')),
    request_id UUID,
    server_id BIGINT NOT NULL REFERENCES servers(id),
    revision BIGINT,
    state TEXT NOT NULL DEFAULT 'pending' CHECK(state IN ('pending','verified','failed','expired')),
    observed_at BIGINT,
    error TEXT,
    dependency_vector JSONB,
    created_at BIGINT NOT NULL,
    UNIQUE(chain_id,generation,stage),
    FOREIGN KEY(chain_id,generation) REFERENCES singbox_ordered_chain_versions(chain_id,generation)
);
CREATE TABLE singbox_resource_mutation_requests (
    request_id UUID PRIMARY KEY,
    request_sha256 TEXT NOT NULL CHECK(request_sha256 ~ '^[0-9a-f]{64}$'),
    receipt JSONB NOT NULL,
    created_at BIGINT NOT NULL
);
CREATE TABLE singbox_path_controller_secrets (
    server_id BIGINT PRIMARY KEY REFERENCES servers(id),
    secret TEXT NOT NULL CHECK(secret ~ '^[0-9a-f]{64}$')
);
CREATE TABLE singbox_deployment_public_projection (
    server_id BIGINT NOT NULL,module TEXT NOT NULL DEFAULT 'singbox',revision BIGINT NOT NULL,node_id BIGINT NOT NULL REFERENCES nodes(id),
    public_fields JSONB NOT NULL CHECK(jsonb_typeof(public_fields)='object'),
    PRIMARY KEY(server_id,module,revision,node_id),
    FOREIGN KEY(server_id,module,revision) REFERENCES deployments(server_id,module,rev)
);
CREATE TABLE singbox_runtime_manifest_facts (
    server_id BIGINT NOT NULL,module TEXT NOT NULL DEFAULT 'singbox',revision BIGINT NOT NULL,
    runtime_version TEXT NOT NULL CHECK(runtime_version='1.14.2'),artifact_sha256 TEXT NOT NULL,artifact JSONB NOT NULL,
    PRIMARY KEY(server_id,module,revision),
    FOREIGN KEY(server_id,module,revision) REFERENCES deployments(server_id,module,rev)
);

-- Initial takeover freezes the old identities without rewriting any deployed bytes.
INSERT INTO singbox_managed_endpoint_versions(id,node_id,server_id,snapshot,semantic_sha256,created_at)
SELECT gen_random_uuid(),n.id,n.server_id,j.snapshot,encode(sha256(convert_to(j.snapshot::text,'UTF8')),'hex'),FLOOR(EXTRACT(EPOCH FROM clock_timestamp()))::bigint
FROM nodes n CROSS JOIN LATERAL (SELECT jsonb_build_object('id',n.id,'name',n.name,'port',n.port,'public_host',n.public_host,'sni',n.sni,'private_key',n.private_key,'public_key',n.public_key,'short_id',n.short_id,'users','[]'::jsonb,'enabled',n.enabled,'settings',n.settings,'protocol_config',n.protocol_config) AS snapshot) j
WHERE EXISTS(SELECT 1 FROM singbox_chains c WHERE c.path_kind='legacy' AND (c.entry_node_id=n.id OR c.exit_node_id=n.id));
INSERT INTO singbox_ordered_chain_versions(chain_id,generation,legacy,entry_endpoint_version,semantic_sha256,capabilities,snapshot,created_at)
SELECT c.id,1,TRUE,ev.id,encode(sha256(convert_to(j.snapshot::text,'UTF8')),'hex'),'{"tcp":true,"udp":true}'::jsonb,j.snapshot,FLOOR(EXTRACT(EPOCH FROM clock_timestamp()))::bigint
FROM singbox_chains c JOIN singbox_managed_endpoint_versions ev ON ev.node_id=c.entry_node_id JOIN singbox_managed_endpoint_versions xv ON xv.node_id=c.exit_node_id
CROSS JOIN LATERAL (SELECT jsonb_build_object('entry',jsonb_build_object('version_id',ev.id,'server_id',ev.server_id,'node',ev.snapshot),'hops',jsonb_build_array(jsonb_build_object('kind','managed','endpoint',jsonb_build_object('version_id',xv.id,'server_id',xv.server_id,'node',xv.snapshot),'relay_uuid',c.relay_uuid)),'legacy_relay_uuid',c.relay_uuid) AS snapshot) j WHERE c.path_kind='legacy';
INSERT INTO singbox_ordered_chain_hops(chain_id,generation,position,kind,endpoint_version_id,managed_node_id,managed_server_id,relay_uuid)
SELECT c.id,1,1,'managed',v.id,v.node_id,v.server_id,c.relay_uuid FROM singbox_chains c JOIN singbox_managed_endpoint_versions v ON v.node_id=c.exit_node_id WHERE c.path_kind='legacy';

CREATE FUNCTION singbox_reject_path_snapshot_change() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'immutable path snapshot' USING ERRCODE='55000';
END;
$$;
CREATE TRIGGER singbox_endpoint_snapshot_immutable BEFORE UPDATE OR DELETE ON singbox_managed_endpoint_versions FOR EACH ROW EXECUTE FUNCTION singbox_reject_path_snapshot_change();
CREATE TRIGGER singbox_path_snapshot_immutable BEFORE UPDATE OR DELETE ON singbox_ordered_chain_versions FOR EACH ROW EXECUTE FUNCTION singbox_reject_path_snapshot_change();
CREATE TRIGGER singbox_ordered_path_hop_immutable BEFORE UPDATE OR DELETE ON singbox_ordered_chain_hops FOR EACH ROW EXECUTE FUNCTION singbox_reject_path_snapshot_change();
CREATE TRIGGER singbox_path_deployment_dependency_immutable BEFORE UPDATE OR DELETE ON singbox_path_deployment_dependencies FOR EACH ROW EXECUTE FUNCTION singbox_reject_path_snapshot_change();
CREATE TRIGGER singbox_path_runtime_requirement_immutable BEFORE UPDATE OR DELETE ON singbox_chain_runtime_requirements FOR EACH ROW EXECUTE FUNCTION singbox_reject_path_snapshot_change();

-- Compose the existing numeric paths with the new UUID ordered lineage.
-- Existing direct grants remain valid for legacy/mixed resources. Ordered entries
-- use their explicit chain grant and reserve retiring entries until cleanup ends.
CREATE OR REPLACE VIEW singbox_desired_accesses AS
    SELECT DISTINCT granted.user_id,granted.node_id FROM (
        SELECT user_id,node_id FROM accesses a WHERE direct_grant AND NOT EXISTS(
            SELECT 1 FROM singbox_chains c WHERE c.path_kind='ordered' AND c.entry_node_id=a.node_id AND (c.deleted_at IS NULL OR c.phase<>'retired'))
        UNION SELECT u.user_id,n.node_id FROM singbox_user_policies u JOIN singbox_policy_nodes n ON n.group_id=u.group_id WHERE NOT EXISTS(
            SELECT 1 FROM singbox_chains c WHERE c.path_kind='ordered' AND c.entry_node_id=n.node_id AND (c.deleted_at IS NULL OR c.phase<>'retired'))
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

CREATE OR REPLACE FUNCTION singbox_eligible_accesses(at_s BIGINT)
RETURNS TABLE (user_id BIGINT,node_id BIGINT,uuid UUID,stat_name TEXT,credential TEXT)
LANGUAGE sql STABLE AS $$
    SELECT a.user_id,a.node_id,a.uuid,a.stat_name,a.credential FROM accesses a
    JOIN singbox_desired_accesses d USING(user_id,node_id)
    JOIN singbox_entitlements(at_s) e ON e.user_id=a.user_id AND e.allowed
    JOIN nodes n ON n.id=a.node_id AND n.enabled
    WHERE NOT EXISTS(SELECT 1 FROM singbox_chains c WHERE c.entry_node_id=n.id AND c.deleted_at IS NULL AND (
        (c.path_kind='legacy' AND EXISTS(SELECT 1 FROM nodes x WHERE x.id=c.exit_node_id AND NOT x.enabled))
        OR (c.path_kind='ordered' AND EXISTS(SELECT 1 FROM singbox_ordered_chain_hops h JOIN nodes x ON x.id=h.managed_node_id
            WHERE h.chain_id=c.id AND h.generation=ANY(ARRAY[c.desired_generation,c.applied_generation,c.candidate_generation,c.recovery_generation]) AND NOT x.enabled))
    ));
$$;
