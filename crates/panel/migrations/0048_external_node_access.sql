CREATE TABLE singbox_external_access_state (
    user_id BIGINT PRIMARY KEY REFERENCES users(id),
    revision BIGINT NOT NULL CHECK (revision > 0)
);

CREATE TABLE singbox_external_accesses (
    user_id BIGINT NOT NULL REFERENCES users(id),
    external_node_id BIGINT NOT NULL,
    source_id BIGINT NOT NULL,
    identity_epoch BIGINT NOT NULL CHECK (identity_epoch > 0),
    node_version_id BIGINT NOT NULL,
    update_mode TEXT NOT NULL CHECK (update_mode IN ('follow_node', 'pinned')),
    created_at BIGINT NOT NULL,
    PRIMARY KEY (user_id, external_node_id),
    FOREIGN KEY (external_node_id, source_id)
        REFERENCES singbox_external_nodes(id, source_id),
    FOREIGN KEY (node_version_id, external_node_id)
        REFERENCES singbox_external_node_versions(id, external_node_id),
    FOREIGN KEY (node_version_id, source_id)
        REFERENCES singbox_external_node_versions(id, source_id)
);
CREATE INDEX singbox_external_access_node ON singbox_external_accesses(external_node_id);
