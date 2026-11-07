-- Numbered sources move into ordered sources only through the explicit
-- `sinan-panel source-migration apply` command (ADR 0085 phase 3, step S1b).
-- This migration adds the bookkeeping and switching points; it moves no data.

CREATE TABLE singbox_source_migration (
    singleton BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (singleton),
    -- NULL while numbered sources are authoritative.
    migrated_at BIGINT,
    report JSONB NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(report)='object')
);
INSERT INTO singbox_source_migration DEFAULT VALUES;

CREATE FUNCTION singbox_sources_migrated() RETURNS BOOLEAN
LANGUAGE sql STABLE AS $$
    SELECT migrated_at IS NOT NULL FROM singbox_source_migration
$$;

CREATE TABLE singbox_source_id_map (
    a_source_id BIGINT PRIMARY KEY REFERENCES singbox_subscription_sources(id),
    b_source_id BIGINT NOT NULL UNIQUE REFERENCES singbox_ordered_subscription_sources(id)
);
CREATE TABLE singbox_source_revision_map (
    a_revision_id BIGINT PRIMARY KEY REFERENCES singbox_source_revisions(id),
    b_revision_id UUID NOT NULL UNIQUE REFERENCES singbox_subscription_source_revisions(id)
);

-- The digest numbered sources stored for the imported original configuration.
ALTER TABLE singbox_ordered_external_node_versions
    ADD COLUMN legacy_config_sha256 TEXT CHECK (legacy_config_sha256 ~ '^[0-9a-f]{64}$');
ALTER TABLE singbox_ordered_external_node_versions
    ADD CONSTRAINT singbox_ordered_external_version_legacy
        CHECK ((legacy_config IS NULL) = (legacy_config_sha256 IS NULL));

-- Catalog metadata and user grants refer to external nodes by public numeric
-- id. Ids come from one sequence, so a node lives in exactly one of the two
-- source tables; neither table ever deletes nodes or versions.
DO $$
DECLARE
    item RECORD;
BEGIN
    FOR item IN
        SELECT conrelid::regclass AS relation, conname FROM pg_constraint
        WHERE contype='f'
          AND conrelid IN ('singbox_node_metadata'::regclass, 'singbox_external_accesses'::regclass)
          AND confrelid IN ('singbox_external_nodes'::regclass, 'singbox_external_node_versions'::regclass)
    LOOP
        EXECUTE format('ALTER TABLE %s DROP CONSTRAINT %I', item.relation, item.conname);
    END LOOP;
END $$;

CREATE FUNCTION singbox_check_external_metadata() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.kind <> 'external'
        OR EXISTS(SELECT 1 FROM singbox_external_nodes WHERE id=NEW.id)
        OR EXISTS(SELECT 1 FROM singbox_ordered_external_nodes WHERE public_id=NEW.id) THEN
        RETURN NEW;
    END IF;
    RAISE EXCEPTION 'external node metadata refers to an unknown node' USING ERRCODE='23503';
END $$;
CREATE TRIGGER singbox_external_metadata_reference
    BEFORE INSERT OR UPDATE ON singbox_node_metadata
    FOR EACH ROW EXECUTE FUNCTION singbox_check_external_metadata();

CREATE FUNCTION singbox_check_external_access() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF EXISTS(
        SELECT 1 FROM singbox_external_nodes n
        JOIN singbox_external_node_versions v ON v.external_node_id=n.id AND v.source_id=n.source_id
        WHERE n.id=NEW.external_node_id AND n.source_id=NEW.source_id AND v.id=NEW.node_version_id
    ) OR EXISTS(
        SELECT 1 FROM singbox_ordered_external_nodes n
        JOIN singbox_ordered_external_node_versions v ON v.node_id=n.id
        WHERE n.public_id=NEW.external_node_id AND n.source_id=NEW.source_id AND v.public_id=NEW.node_version_id
    ) THEN
        RETURN NEW;
    END IF;
    RAISE EXCEPTION 'external access refers to an unknown node version' USING ERRCODE='23503';
END $$;
CREATE TRIGGER singbox_external_access_reference
    BEFORE INSERT OR UPDATE ON singbox_external_accesses
    FOR EACH ROW EXECUTE FUNCTION singbox_check_external_access();

-- Once migrated, numbered sources are a read-only archive: mixed chains still
-- read their versions, and a rollback must find them exactly as they were.
CREATE FUNCTION singbox_reject_numbered_source_write() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF singbox_sources_migrated() THEN
        RAISE EXCEPTION 'numbered subscription sources are read-only after the source migration'
            USING ERRCODE='55000';
    END IF;
    RETURN COALESCE(NEW, OLD);
END $$;
DO $$
DECLARE
    relation TEXT;
BEGIN
    FOREACH relation IN ARRAY ARRAY[
        'singbox_subscription_sources', 'singbox_source_revisions', 'singbox_external_nodes',
        'singbox_external_node_versions', 'singbox_source_jobs', 'singbox_source_previews'
    ] LOOP
        EXECUTE format(
            'CREATE TRIGGER singbox_numbered_source_archive BEFORE INSERT OR UPDATE OR DELETE ON %I '
            'FOR EACH ROW EXECUTE FUNCTION singbox_reject_numbered_source_write()', relation);
    END LOOP;
END $$;

-- Catalog, grants and subscriptions read external nodes through these views:
-- numbered sources before the migration, ordered sources after it. Ids are
-- the public numeric ids in both cases. A numbered node is present when the
-- latest revision of its identity epoch listed it; the same rule is applied
-- to ordered nodes, so an imported node keeps its state.
CREATE VIEW singbox_external_node_states AS
    SELECT n.id, n.source_id, n.identity_epoch, n.current_version_id, n.name, n.adopted,
           n.present, n.identity_unique, s.name AS source_name, s.identity_epoch AS source_epoch,
           s.archived AS source_archived, s.deleted_at AS source_deleted_at,
           s.last_error AS source_last_error, s.settings_revision AS source_settings_revision
    FROM singbox_external_nodes n
    JOIN singbox_subscription_sources s ON s.id=n.source_id
    WHERE NOT singbox_sources_migrated()
    UNION ALL
    SELECT n.public_id, n.source_id, n.identity_epoch, v.public_id,
           COALESCE(m.public_preview->>'name', v.public_preview->>'name', ''), n.adopted,
           COALESCE(n.last_seen_revision=(
               SELECT r.id FROM singbox_subscription_source_revisions r
               WHERE r.source_id=n.source_id AND r.identity_epoch=n.identity_epoch
               ORDER BY r.generation DESC LIMIT 1
           ), FALSE),
           n.identity_state='unique', s.name, s.identity_epoch,
           s.archived, s.deleted_at, s.last_error->>'kind', s.settings_revision
    FROM singbox_ordered_external_nodes n
    JOIN singbox_ordered_subscription_sources s ON s.id=n.source_id
    LEFT JOIN singbox_ordered_external_node_versions v ON v.id=n.latest_version
    LEFT JOIN singbox_subscription_revision_nodes m
        ON m.node_id=n.id AND m.source_revision_id=n.last_seen_revision
    WHERE singbox_sources_migrated();

CREATE VIEW singbox_external_version_configs AS
    SELECT v.id, v.external_node_id, v.source_id, v.identity_epoch, v.parser_version,
           FALSE AS ordered, v.config_json, v.config_sha256,
           NULL::jsonb AS normalized_config, NULL::text AS content_digest
    FROM singbox_external_node_versions v
    WHERE NOT singbox_sources_migrated()
    UNION ALL
    SELECT v.public_id, n.public_id, n.source_id, n.identity_epoch, r.parser_version,
           TRUE, v.legacy_config, v.legacy_config_sha256, v.normalized_config, v.content_digest
    FROM singbox_ordered_external_node_versions v
    JOIN singbox_ordered_external_nodes n ON n.id=v.node_id
    JOIN singbox_subscription_source_revisions r ON r.id=v.source_revision_id
    WHERE singbox_sources_migrated();
