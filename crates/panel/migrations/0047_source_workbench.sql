ALTER TABLE singbox_subscription_sources
    ADD COLUMN auto_refresh BOOLEAN NOT NULL DEFAULT TRUE,
    ADD COLUMN user_agent TEXT NOT NULL DEFAULT 'Sinan-subscription-import/1'
        CHECK (octet_length(user_agent) BETWEEN 1 AND 256),
    ADD COLUMN traffic JSONB NOT NULL DEFAULT '{}',
    ADD COLUMN changes JSONB NOT NULL DEFAULT '{"added":0,"updated":0,"missing":0,"unsupported":0}';

ALTER TABLE singbox_external_nodes ADD COLUMN adopted BOOLEAN NOT NULL DEFAULT FALSE;
UPDATE singbox_external_nodes SET adopted=TRUE;

CREATE TABLE singbox_source_previews (
    id UUID PRIMARY KEY,
    admin_id BIGINT NOT NULL REFERENCES admins(id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK (kind IN ('url','inline')),
    secret_url TEXT,
    secret_authorization TEXT,
    secret_body BYTEA NOT NULL CHECK (octet_length(secret_body) BETWEEN 1 AND 2097152),
    user_agent TEXT NOT NULL CHECK (octet_length(user_agent) BETWEEN 1 AND 256),
    parser_version TEXT NOT NULL,
    traffic JSONB NOT NULL DEFAULT '{}',
    created_at BIGINT NOT NULL,
    expires_at BIGINT NOT NULL,
    CHECK (expires_at > created_at AND expires_at <= created_at + 600),
    CHECK ((kind='url' AND secret_url IS NOT NULL) OR (kind='inline' AND secret_url IS NULL)),
    CHECK (secret_url IS NULL OR octet_length(secret_url) <= 8192),
    CHECK (secret_authorization IS NULL OR octet_length(secret_authorization) <= 8192)
);
CREATE INDEX singbox_source_preview_expiry ON singbox_source_previews(expires_at);
