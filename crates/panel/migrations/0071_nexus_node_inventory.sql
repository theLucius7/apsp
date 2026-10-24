-- Imported inventory is independent of desired runtime configuration. Creating
-- these rows must never enable a server plugin or schedule a publication.
CREATE TABLE singbox_nexus_node_imports (
    source_id TEXT PRIMARY KEY,
    digest TEXT NOT NULL CHECK (length(digest) = 64),
    infrastructure_digest TEXT NOT NULL CHECK (length(infrastructure_digest) = 64),
    exported_at BIGINT NOT NULL,
    imported_at BIGINT NOT NULL,
    node_count INTEGER NOT NULL CHECK (node_count > 0)
);

CREATE TABLE singbox_nexus_nodes (
    id BIGSERIAL PRIMARY KEY,
    source_id TEXT NOT NULL REFERENCES singbox_nexus_node_imports(source_id),
    legacy_node_id BIGINT NOT NULL CHECK (legacy_node_id > 0),
    legacy_machine_id BIGINT NOT NULL CHECK (legacy_machine_id > 0),
    legacy_host_id BIGINT NOT NULL CHECK (legacy_host_id > 0),
    server_id BIGINT NOT NULL REFERENCES servers(id),
    credential_id UUID NOT NULL UNIQUE REFERENCES credential_entries(id),
    display JSONB NOT NULL CHECK (jsonb_typeof(display) = 'object'),
    UNIQUE(source_id, legacy_node_id)
);
CREATE INDEX singbox_nexus_nodes_server ON singbox_nexus_nodes(server_id);
