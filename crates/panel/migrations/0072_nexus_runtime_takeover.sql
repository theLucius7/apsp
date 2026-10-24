CREATE TABLE singbox_nexus_runtime_hosts (
    server_id BIGINT PRIMARY KEY REFERENCES servers(id),
    source_id TEXT NOT NULL REFERENCES singbox_nexus_node_imports(source_id),
    credential_id UUID NOT NULL UNIQUE REFERENCES credential_entries(id),
    digest TEXT NOT NULL CHECK(length(digest)=64),
    state TEXT NOT NULL DEFAULT 'prepared' CHECK(state IN ('prepared','active','paused')),
    membership_digest TEXT,
    created_at BIGINT NOT NULL,
    activated_at BIGINT,
    evidence JSONB NOT NULL DEFAULT '{}'
);
CREATE TABLE singbox_nexus_runtime_nodes (
    inventory_id BIGINT PRIMARY KEY REFERENCES singbox_nexus_nodes(id),
    node_id BIGINT NOT NULL UNIQUE REFERENCES nodes(id),
    server_id BIGINT NOT NULL REFERENCES singbox_nexus_runtime_hosts(server_id),
    legacy_group_ids BIGINT[] NOT NULL,
    rate NUMERIC(12,6) NOT NULL CHECK(rate>=0)
);

-- Generic mutation paths must not rotate imported wire identities or publish
-- a second configuration on a host under the migration controller.
CREATE FUNCTION singbox_nexus_guard_node() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF current_setting('apsp.nexus_runtime_write',TRUE) IS DISTINCT FROM 'yes' AND
       EXISTS(SELECT 1 FROM singbox_nexus_runtime_hosts WHERE server_id IN (NEW.server_id,OLD.server_id)) THEN
        RAISE EXCEPTION 'Nexus runtime nodes are managed by their preserved configuration';
    END IF;
    IF TG_OP='DELETE' THEN RETURN OLD; ELSE RETURN NEW; END IF;
END $$;
CREATE TRIGGER singbox_nexus_guard_node BEFORE INSERT OR UPDATE OR DELETE ON nodes
FOR EACH ROW EXECUTE FUNCTION singbox_nexus_guard_node();

-- Raw device counters remain immutable. Only this durable delivery queue
-- carries legacy billing multipliers into the compatibility meter.
CREATE TABLE singbox_nexus_usage_deliveries (
    id BIGSERIAL PRIMARY KEY,
    server_id BIGINT NOT NULL,
    epoch UUID NOT NULL,
    seq NUMERIC(20,0) NOT NULL,
    stat_name TEXT NOT NULL,
    user_id BIGINT NOT NULL REFERENCES users(id),
    uplink NUMERIC(30,0) NOT NULL,
    downlink NUMERIC(30,0) NOT NULL,
    rate NUMERIC(12,6) NOT NULL,
    period_start BIGINT NOT NULL,
    period_end BIGINT NOT NULL,
    delivered_at BIGINT,
    UNIQUE(server_id,epoch,seq,stat_name),
    FOREIGN KEY(server_id,epoch,seq) REFERENCES usage_batches(server_id,epoch,seq)
);
CREATE INDEX singbox_nexus_usage_pending ON singbox_nexus_usage_deliveries(id) WHERE delivered_at IS NULL;

-- Imported users are inherited from the source permission-group mapping. A
-- generic grant must not report success while silently generating a new UUID.
CREATE FUNCTION singbox_nexus_guard_access() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF EXISTS(SELECT 1 FROM singbox_nexus_runtime_nodes WHERE node_id IN (NEW.node_id,OLD.node_id)) THEN
        RAISE EXCEPTION 'Nexus runtime access is inherited from its legacy permission groups';
    END IF;
    IF TG_OP='DELETE' THEN RETURN OLD; ELSE RETURN NEW; END IF;
END $$;
CREATE TRIGGER singbox_nexus_guard_access BEFORE INSERT OR UPDATE OR DELETE ON accesses
FOR EACH ROW EXECUTE FUNCTION singbox_nexus_guard_access();
