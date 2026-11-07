CREATE TABLE singbox_ordered_subscription_sources (
    id BIGSERIAL PRIMARY KEY,
    name TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('url','inline')),
    host TEXT,
    input_config JSONB NOT NULL CHECK (jsonb_typeof(input_config)='object'),
    settings_revision BIGINT NOT NULL DEFAULT 1 CHECK (settings_revision>0),
    identity_epoch BIGINT NOT NULL DEFAULT 1 CHECK (identity_epoch>0),
    archived BOOLEAN NOT NULL DEFAULT FALSE,
    deleted_at BIGINT,
    refresh_interval_secs BIGINT NOT NULL CHECK (
        (kind='inline' AND refresh_interval_secs=0) OR
        (kind='url' AND refresh_interval_secs BETWEEN 3600 AND 604800)
    ),
    next_refresh_at BIGINT,
    last_attempt_at BIGINT,
    last_success_at BIGINT,
    current_success_revision UUID,
    last_error JSONB,
    conditional_etag TEXT,
    conditional_last_modified TEXT,
    conditional_settings_revision BIGINT,
    conditional_identity_epoch BIGINT,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL
);
CREATE INDEX singbox_subscription_sources_due_idx ON singbox_ordered_subscription_sources(next_refresh_at,id)
    WHERE deleted_at IS NULL AND NOT archived;

CREATE TABLE singbox_subscription_source_jobs (
    id UUID PRIMARY KEY CHECK (id<>'00000000-0000-0000-0000-000000000000'),
    source_id BIGINT NOT NULL REFERENCES singbox_ordered_subscription_sources(id),
    settings_revision BIGINT NOT NULL CHECK (settings_revision>0),
    identity_epoch BIGINT NOT NULL CHECK (identity_epoch>0),
    parser_version TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('queued','running','cancelling','succeeded','unchanged','failed','cancelled','superseded')),
    stage TEXT NOT NULL CHECK (stage IN ('queued','fetch','parse','store','done')),
    claim_token UUID,
    deadline_at BIGINT,
    created_at BIGINT NOT NULL,
    started_at BIGINT,
    finished_at BIGINT,
    source_revision_id UUID,
    error JSONB,
    CHECK ((status IN ('running','cancelling'))=(claim_token IS NOT NULL)),
    CHECK (status NOT IN ('running','cancelling') OR deadline_at IS NOT NULL)
);
CREATE UNIQUE INDEX singbox_subscription_source_one_active_job_idx
    ON singbox_subscription_source_jobs(source_id) WHERE status IN ('queued','running','cancelling');
CREATE INDEX singbox_subscription_source_jobs_due_idx ON singbox_subscription_source_jobs(status,created_at,id);
CREATE INDEX singbox_subscription_source_jobs_history_idx ON singbox_subscription_source_jobs(source_id,created_at DESC,id);

CREATE TABLE singbox_subscription_source_revisions (
    id UUID PRIMARY KEY,
    generation BIGSERIAL NOT NULL UNIQUE,
    source_id BIGINT NOT NULL REFERENCES singbox_ordered_subscription_sources(id),
    job_id UUID NOT NULL UNIQUE REFERENCES singbox_subscription_source_jobs(id),
    settings_revision BIGINT NOT NULL CHECK (settings_revision>0),
    identity_epoch BIGINT NOT NULL CHECK (identity_epoch>0),
    parser_version TEXT NOT NULL,
    format TEXT NOT NULL CHECK (format IN ('uri_list','base64_uri_list','sing_box_json','clash_yaml')),
    raw_digest TEXT NOT NULL CHECK (raw_digest ~ '^[0-9a-f]{64}$'),
    parsed_at BIGINT NOT NULL,
    counts JSONB NOT NULL CHECK (jsonb_typeof(counts)='object'),
    warnings JSONB NOT NULL CHECK (jsonb_typeof(warnings)='array')
);
CREATE INDEX singbox_subscription_source_revisions_history_idx
    ON singbox_subscription_source_revisions(source_id,generation DESC);
ALTER TABLE singbox_ordered_subscription_sources ADD FOREIGN KEY(current_success_revision)
    REFERENCES singbox_subscription_source_revisions(id);
ALTER TABLE singbox_subscription_source_jobs ADD FOREIGN KEY(source_revision_id)
    REFERENCES singbox_subscription_source_revisions(id);

CREATE TABLE singbox_ordered_external_nodes (
    id UUID PRIMARY KEY,
    source_id BIGINT NOT NULL REFERENCES singbox_ordered_subscription_sources(id),
    identity_epoch BIGINT NOT NULL CHECK (identity_epoch>0),
    identity_key TEXT,
    identity_state TEXT NOT NULL CHECK (identity_state IN ('unique','ambiguous','unresolved')),
    latest_version UUID,
    last_seen_revision UUID REFERENCES singbox_subscription_source_revisions(id),
    created_at BIGINT NOT NULL,
    CHECK ((identity_state='unique')=(identity_key IS NOT NULL))
);
CREATE UNIQUE INDEX singbox_external_nodes_identity_idx
    ON singbox_ordered_external_nodes(source_id,identity_epoch,identity_key) WHERE identity_state='unique';
CREATE INDEX singbox_external_nodes_source_idx ON singbox_ordered_external_nodes(source_id,identity_epoch,id);

CREATE TABLE singbox_ordered_external_node_versions (
    id UUID PRIMARY KEY,
    node_id UUID NOT NULL REFERENCES singbox_ordered_external_nodes(id),
    source_revision_id UUID NOT NULL REFERENCES singbox_subscription_source_revisions(id),
    normalized_config JSONB,
    content_digest TEXT CHECK (content_digest ~ '^[0-9a-f]{64}$'),
    public_preview JSONB NOT NULL CHECK (jsonb_typeof(public_preview)='object'),
    capabilities JSONB NOT NULL CHECK (jsonb_typeof(capabilities)='object'),
    supported BOOLEAN NOT NULL,
    reasons JSONB NOT NULL CHECK (jsonb_typeof(reasons)='array'),
    CHECK (supported=(normalized_config IS NOT NULL AND content_digest IS NOT NULL))
);
ALTER TABLE singbox_ordered_external_nodes ADD FOREIGN KEY(latest_version) REFERENCES singbox_ordered_external_node_versions(id);

CREATE TABLE singbox_subscription_revision_nodes (
    source_revision_id UUID NOT NULL REFERENCES singbox_subscription_source_revisions(id),
    ordinal INTEGER NOT NULL CHECK (ordinal>=0 AND ordinal<5000),
    node_id UUID NOT NULL REFERENCES singbox_ordered_external_nodes(id),
    version_id UUID NOT NULL REFERENCES singbox_ordered_external_node_versions(id),
    public_preview JSONB NOT NULL CHECK (jsonb_typeof(public_preview)='object'),
    identity_state TEXT NOT NULL CHECK (identity_state IN ('unique','ambiguous','unresolved')),
    PRIMARY KEY(source_revision_id,ordinal),
    UNIQUE(source_revision_id,node_id)
);

-- Receipts and immutable history survive source deletion. Neither stores request secrets.
CREATE TABLE singbox_subscription_source_requests (
    request_id UUID PRIMARY KEY CHECK (request_id<>'00000000-0000-0000-0000-000000000000'),
    operation TEXT NOT NULL,
    source_id BIGINT,
    request_sha256 TEXT NOT NULL CHECK (request_sha256 ~ '^[0-9a-f]{64}$'),
    receipt JSONB NOT NULL CHECK (jsonb_typeof(receipt)='object'),
    created_at BIGINT NOT NULL
);

CREATE FUNCTION singbox_reject_subscription_snapshot_change() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'immutable subscription snapshot' USING ERRCODE='55000';
END;
$$;
CREATE TRIGGER singbox_subscription_revision_immutable BEFORE UPDATE OR DELETE
    ON singbox_subscription_source_revisions FOR EACH ROW EXECUTE FUNCTION singbox_reject_subscription_snapshot_change();
CREATE TRIGGER singbox_external_version_immutable BEFORE UPDATE OR DELETE
    ON singbox_ordered_external_node_versions FOR EACH ROW EXECUTE FUNCTION singbox_reject_subscription_snapshot_change();
CREATE TRIGGER singbox_subscription_membership_immutable BEFORE UPDATE OR DELETE
    ON singbox_subscription_revision_nodes FOR EACH ROW EXECUTE FUNCTION singbox_reject_subscription_snapshot_change();
