-- Ordered sources gain the settings and adoption state of numbered sources
-- (ADR 0085 phase 3, step S1a). Existing rows keep their current behaviour:
-- no User-Agent header, automatic refresh on, and no node adopted into the
-- catalog until an administrator does so explicitly.

ALTER TABLE singbox_ordered_subscription_sources
    ADD COLUMN user_agent TEXT CHECK (user_agent IS NULL OR octet_length(user_agent) BETWEEN 1 AND 256),
    ADD COLUMN auto_refresh BOOLEAN NOT NULL DEFAULT TRUE,
    ADD COLUMN traffic JSONB NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(traffic)='object'),
    ADD COLUMN changes JSONB NOT NULL DEFAULT '{"added":0,"updated":0,"missing":0,"unsupported":0}'::jsonb
        CHECK (jsonb_typeof(changes)='object');

-- Accept the refresh periods numbered sources already allow.
DO $$
DECLARE
    old_name TEXT;
BEGIN
    SELECT conname INTO STRICT old_name FROM pg_constraint
    WHERE conrelid='singbox_ordered_subscription_sources'::regclass AND contype='c'
      AND pg_get_constraintdef(oid) LIKE '%refresh_interval_secs%3600%';
    EXECUTE format('ALTER TABLE singbox_ordered_subscription_sources DROP CONSTRAINT %I', old_name);
END $$;
ALTER TABLE singbox_ordered_subscription_sources ADD CONSTRAINT singbox_ordered_source_refresh_interval CHECK (
    (kind='inline' AND refresh_interval_secs=0) OR
    (kind='url' AND refresh_interval_secs BETWEEN 300 AND 2592000)
);

-- Public numeric ids share the numbered-source sequences, so ids imported
-- from numbered sources later can never collide with ids issued here.
ALTER TABLE singbox_ordered_external_nodes
    ADD COLUMN adopted BOOLEAN NOT NULL DEFAULT FALSE,
    ADD COLUMN public_id BIGINT NOT NULL DEFAULT nextval('singbox_external_nodes_id_seq'::regclass);
ALTER TABLE singbox_ordered_external_nodes
    ADD CONSTRAINT singbox_ordered_external_node_public_id UNIQUE (public_id);

ALTER TABLE singbox_ordered_external_node_versions
    ADD COLUMN public_id BIGINT NOT NULL DEFAULT nextval('singbox_external_node_versions_id_seq'::regclass),
    -- Only versions imported from numbered sources carry their original
    -- outbound, so migration does not change rendered subscriptions.
    ADD COLUMN legacy_config JSONB CHECK (legacy_config IS NULL OR jsonb_typeof(legacy_config)='object');
ALTER TABLE singbox_ordered_external_node_versions
    ADD CONSTRAINT singbox_ordered_external_version_public_id UNIQUE (public_id);

CREATE TABLE singbox_ordered_source_previews (
    id UUID PRIMARY KEY CHECK (id<>'00000000-0000-0000-0000-000000000000'),
    admin_id BIGINT NOT NULL REFERENCES admins(id) ON DELETE CASCADE,
    input_config JSONB NOT NULL CHECK (jsonb_typeof(input_config)='object'),
    body BYTEA NOT NULL CHECK (octet_length(body) BETWEEN 1 AND 2097152),
    user_agent TEXT CHECK (user_agent IS NULL OR octet_length(user_agent) BETWEEN 1 AND 256),
    parser_version TEXT NOT NULL,
    traffic JSONB NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(traffic)='object'),
    created_at BIGINT NOT NULL,
    expires_at BIGINT NOT NULL CHECK (expires_at>created_at AND expires_at<=created_at+600)
);
CREATE INDEX singbox_ordered_source_preview_expiry ON singbox_ordered_source_previews(expires_at);
