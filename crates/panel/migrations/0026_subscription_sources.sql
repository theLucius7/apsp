CREATE TABLE singbox_subscription_sources (
    id BIGSERIAL PRIMARY KEY,
    name TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('url', 'inline')),
    secret_url TEXT,
    secret_authorization TEXT,
    secret_content TEXT,
    source_host TEXT,
    settings_revision BIGINT NOT NULL DEFAULT 1 CHECK (settings_revision > 0),
    identity_epoch BIGINT NOT NULL DEFAULT 1 CHECK (identity_epoch > 0),
    refresh_interval_seconds BIGINT NOT NULL DEFAULT 86400
        CHECK (refresh_interval_seconds BETWEEN 300 AND 2592000),
    archived BOOLEAN NOT NULL DEFAULT FALSE,
    deleted_at BIGINT,
    current_revision_id BIGINT,
    last_attempt_at BIGINT,
    last_success_at BIGINT,
    next_refresh_at BIGINT,
    last_error TEXT,
    etag TEXT,
    last_modified TEXT,
    cache_settings_revision BIGINT,
    cache_identity_epoch BIGINT,
    created_at BIGINT NOT NULL,
    CHECK ((kind = 'url' AND secret_url IS NOT NULL AND secret_content IS NULL)
        OR (kind = 'inline' AND secret_url IS NULL AND secret_content IS NOT NULL)),
    CHECK (secret_url IS NULL OR octet_length(secret_url) <= 8192),
    CHECK (secret_authorization IS NULL OR octet_length(secret_authorization) <= 8192),
    CHECK (secret_content IS NULL OR octet_length(secret_content) <= 2097152)
);

CREATE TABLE singbox_source_revisions (
    id BIGSERIAL PRIMARY KEY,
    source_id BIGINT NOT NULL REFERENCES singbox_subscription_sources(id),
    settings_revision BIGINT NOT NULL,
    identity_epoch BIGINT NOT NULL,
    parser_version TEXT NOT NULL,
    body_sha256 TEXT NOT NULL,
    format TEXT NOT NULL,
    supported_count INTEGER NOT NULL CHECK (supported_count BETWEEN 0 AND 5000),
    unsupported_count INTEGER NOT NULL CHECK (unsupported_count BETWEEN 0 AND 5000),
    rejected_nodes JSONB NOT NULL DEFAULT '[]',
    fetched_at BIGINT NOT NULL,
    UNIQUE(id, source_id)
);

ALTER TABLE singbox_subscription_sources ADD CONSTRAINT source_current_revision
    FOREIGN KEY (current_revision_id, id) REFERENCES singbox_source_revisions(id, source_id);

CREATE TABLE singbox_external_nodes (
    id BIGSERIAL PRIMARY KEY,
    source_id BIGINT NOT NULL REFERENCES singbox_subscription_sources(id),
    identity_epoch BIGINT NOT NULL,
    identity_key TEXT NOT NULL,
    name TEXT NOT NULL,
    present BOOLEAN NOT NULL DEFAULT TRUE,
    identity_unique BOOLEAN NOT NULL DEFAULT TRUE,
    current_version_id BIGINT,
    last_seen_revision_id BIGINT REFERENCES singbox_source_revisions(id),
    UNIQUE (source_id, identity_epoch, identity_key),
    UNIQUE (id, source_id)
);

CREATE TABLE singbox_external_node_versions (
    id BIGSERIAL PRIMARY KEY,
    external_node_id BIGINT NOT NULL,
    source_id BIGINT NOT NULL,
    source_revision_id BIGINT NOT NULL,
    identity_epoch BIGINT NOT NULL,
    parser_version TEXT NOT NULL,
    name TEXT NOT NULL,
    config_json JSONB NOT NULL,
    config_sha256 TEXT NOT NULL,
    capabilities_json JSONB NOT NULL,
    created_at BIGINT NOT NULL,
    FOREIGN KEY (external_node_id, source_id) REFERENCES singbox_external_nodes(id, source_id),
    FOREIGN KEY (source_revision_id, source_id) REFERENCES singbox_source_revisions(id, source_id),
    UNIQUE (id, external_node_id),
    UNIQUE (id, source_id)
);
ALTER TABLE singbox_external_nodes ADD CONSTRAINT external_current_version
    FOREIGN KEY (current_version_id, id) REFERENCES singbox_external_node_versions(id, external_node_id);

CREATE TABLE singbox_source_jobs (
    id UUID PRIMARY KEY,
    source_id BIGINT NOT NULL REFERENCES singbox_subscription_sources(id),
    settings_revision BIGINT NOT NULL,
    identity_epoch BIGINT NOT NULL,
    parser_version TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('queued', 'running', 'succeeded', 'failed', 'cancelled', 'superseded')),
    phase TEXT NOT NULL,
    error_code TEXT,
    result_revision_id BIGINT REFERENCES singbox_source_revisions(id),
    created_at BIGINT NOT NULL,
    started_at BIGINT,
    finished_at BIGINT,
    lease_expires_at BIGINT
);
CREATE UNIQUE INDEX singbox_source_single_job ON singbox_source_jobs(source_id)
    WHERE state IN ('queued', 'running');
CREATE INDEX singbox_source_jobs_queue ON singbox_source_jobs(created_at) WHERE state = 'queued';
CREATE INDEX singbox_source_refresh_due ON singbox_subscription_sources(next_refresh_at)
    WHERE kind = 'url' AND NOT archived AND deleted_at IS NULL;
CREATE INDEX singbox_external_node_source ON singbox_external_nodes(source_id, identity_epoch);
CREATE INDEX singbox_external_node_versions_node ON singbox_external_node_versions(external_node_id, id);

CREATE FUNCTION singbox_immutable_source_version() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'subscription versions are immutable';
END;
$$;
CREATE TRIGGER immutable_source_revision BEFORE UPDATE OR DELETE ON singbox_source_revisions
    FOR EACH ROW EXECUTE FUNCTION singbox_immutable_source_version();
CREATE TRIGGER immutable_external_version BEFORE UPDATE OR DELETE ON singbox_external_node_versions
    FOR EACH ROW EXECUTE FUNCTION singbox_immutable_source_version();
