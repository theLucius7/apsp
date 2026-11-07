ALTER TABLE nodes ADD COLUMN enabled BOOLEAN NOT NULL DEFAULT TRUE;
ALTER TABLE nodes ADD COLUMN settings JSONB NOT NULL DEFAULT '{}'::jsonb
    CHECK (jsonb_typeof(settings) = 'object');

-- Pausing a node preserves grants and the ledger but removes current eligibility.
-- A paused exit must never turn its entry into a direct connection.
CREATE OR REPLACE FUNCTION singbox_eligible_accesses(at_s BIGINT)
RETURNS TABLE (user_id BIGINT, node_id BIGINT, uuid UUID, stat_name TEXT, credential TEXT)
LANGUAGE sql STABLE AS $$
    SELECT a.user_id, a.node_id, a.uuid, a.stat_name, a.credential FROM accesses a
    JOIN singbox_desired_accesses d USING (user_id, node_id)
    JOIN singbox_entitlements(at_s) e ON e.user_id=a.user_id AND e.allowed
    JOIN nodes n ON n.id=a.node_id AND n.enabled
    WHERE NOT EXISTS (
        SELECT 1 FROM singbox_chains c JOIN nodes exit_node ON exit_node.id=c.exit_node_id
        WHERE c.entry_node_id=n.id AND NOT exit_node.enabled
    );
$$;
