-- Proxy users get nodes, chains and external nodes only through policy groups
-- (ADR 0112). Every existing per-user grant becomes a group, so nobody gains or
-- loses access: users whose grants were identical share one group.

CREATE TABLE singbox_policy_external_nodes (
    group_id BIGINT NOT NULL REFERENCES singbox_policy_groups(id) ON DELETE CASCADE,
    external_node_id BIGINT NOT NULL,
    identity_epoch BIGINT NOT NULL CHECK (identity_epoch > 0),
    node_version_id BIGINT NOT NULL,
    update_mode TEXT NOT NULL CHECK (update_mode IN ('follow_node', 'pinned')),
    created_at BIGINT NOT NULL,
    PRIMARY KEY (group_id, external_node_id)
);
CREATE INDEX singbox_policy_external_node ON singbox_policy_external_nodes(external_node_id);

-- External ids live in either the numbered or the ordered source tables, so the
-- reference is checked the same way the per-user grant used to be.
CREATE FUNCTION singbox_check_policy_external() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF EXISTS(
        SELECT 1 FROM singbox_external_node_versions v
        WHERE v.id=NEW.node_version_id AND v.external_node_id=NEW.external_node_id
    ) OR EXISTS(
        SELECT 1 FROM singbox_ordered_external_nodes n
        JOIN singbox_ordered_external_node_versions v ON v.node_id=n.id
        WHERE n.public_id=NEW.external_node_id AND v.public_id=NEW.node_version_id
    ) THEN
        RETURN NEW;
    END IF;
    RAISE EXCEPTION 'policy group external node refers to an unknown node version' USING ERRCODE='23503';
END $$;
CREATE TRIGGER singbox_policy_external_reference
    BEFORE INSERT OR UPDATE ON singbox_policy_external_nodes
    FOR EACH ROW EXECUTE FUNCTION singbox_check_policy_external();

-- What each live user was granted directly and that still had an effect: plain
-- nodes, the entry of a two-hop or mixed chain (granted through its chain now),
-- and external nodes. A direct grant on an ordered chain entry never counted.
CREATE TEMP TABLE converted_grants ON COMMIT DROP AS
    SELECT a.user_id, 'node'::TEXT AS kind, a.node_id AS id,
           NULL::BIGINT AS epoch, NULL::BIGINT AS version, NULL::TEXT AS mode
    FROM accesses a
    JOIN users u ON u.id=a.user_id AND u.deleted_at IS NULL
    JOIN nodes n ON n.id=a.node_id AND n.deleted_at IS NULL
    JOIN servers s ON s.id=n.server_id AND s.deleted_at IS NULL
    WHERE a.direct_grant AND NOT EXISTS(
        SELECT 1 FROM singbox_chains c WHERE c.entry_node_id=a.node_id
        AND (c.deleted_at IS NULL OR (c.path_kind='ordered' AND c.phase<>'retired')))
    UNION
    SELECT a.user_id, 'chain', c.id, NULL, NULL, NULL
    FROM accesses a
    JOIN users u ON u.id=a.user_id AND u.deleted_at IS NULL
    JOIN singbox_chains c ON c.entry_node_id=a.node_id AND c.deleted_at IS NULL
        AND c.path_kind IN ('legacy', 'mixed')
    WHERE a.direct_grant
    UNION
    SELECT x.user_id, 'external', x.external_node_id, x.identity_epoch, x.node_version_id, x.update_mode
    FROM singbox_external_accesses x
    JOIN users u ON u.id=x.user_id AND u.deleted_at IS NULL;

DO $$
DECLARE
    item RECORD;
    created BIGINT;
BEGIN
    FOR item IN
        SELECT s.signature, array_agg(s.user_id ORDER BY s.user_id) AS users,
               string_agg(s.name, '、' ORDER BY s.user_id) AS names
        FROM (
            SELECT g.user_id, u.name, string_agg(
                concat_ws(':', g.kind, g.id, g.epoch, g.version, g.mode), ','
                ORDER BY g.kind, g.id) AS signature
            FROM converted_grants g JOIN users u ON u.id=g.user_id
            GROUP BY g.user_id, u.name
        ) s
        GROUP BY s.signature
        ORDER BY min(s.user_id)
    LOOP
        INSERT INTO singbox_policy_groups(name)
            VALUES (left('原单独授权 · ' || item.names, 128)) RETURNING id INTO created;
        INSERT INTO singbox_policy_nodes(group_id, node_id)
            SELECT created, g.id FROM converted_grants g WHERE g.user_id=item.users[1] AND g.kind='node';
        INSERT INTO singbox_policy_chains(group_id, chain_id)
            SELECT created, g.id FROM converted_grants g WHERE g.user_id=item.users[1] AND g.kind='chain';
        INSERT INTO singbox_policy_external_nodes(group_id, external_node_id, identity_epoch, node_version_id, update_mode, created_at)
            SELECT created, g.id, g.epoch, g.version, g.mode, EXTRACT(EPOCH FROM now())::BIGINT
            FROM converted_grants g WHERE g.user_id=item.users[1] AND g.kind='external';
        INSERT INTO singbox_user_policies(user_id, group_id)
            SELECT unnest(item.users), created;
    END LOOP;
END $$;

-- Credentials stay in `accesses`; which nodes a user should have now comes from
-- groups alone.
CREATE OR REPLACE VIEW singbox_desired_accesses AS
    SELECT DISTINCT granted.user_id,granted.node_id FROM (
        SELECT u.user_id,n.node_id FROM singbox_user_policies u JOIN singbox_policy_nodes n ON n.group_id=u.group_id WHERE NOT EXISTS(
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

ALTER TABLE accesses DROP COLUMN direct_grant;
DROP TABLE singbox_external_accesses;
DROP TABLE singbox_external_access_state;
DROP FUNCTION singbox_check_external_access();
