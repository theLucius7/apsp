CREATE TABLE server_plugins (
    server_id BIGINT NOT NULL REFERENCES servers(id),
    plugin TEXT NOT NULL,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    source TEXT NOT NULL CHECK (source IN ('administrator', 'agent_capability', 'legacy_nodes', 'legacy_deployments')),
    enabled_at BIGINT NOT NULL,
    PRIMARY KEY (server_id, plugin)
);

-- Preserve existing proxy deployments without recreating any business records.
INSERT INTO server_plugins(server_id,plugin,source,enabled_at)
SELECT s.id,'sing-box',
    CASE WHEN s.capabilities ? 'singbox' THEN 'agent_capability'
         WHEN EXISTS(SELECT 1 FROM nodes n WHERE n.server_id=s.id) THEN 'legacy_nodes'
         ELSE 'legacy_deployments' END,
    FLOOR(EXTRACT(EPOCH FROM clock_timestamp()))::bigint
FROM servers s WHERE s.deleted_at IS NULL AND (
    s.capabilities ? 'singbox'
    OR EXISTS(SELECT 1 FROM nodes n WHERE n.server_id=s.id)
    OR EXISTS(SELECT 1 FROM deployments d WHERE d.server_id=s.id AND d.module='singbox')
);
