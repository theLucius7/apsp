CREATE TABLE passkey_accounts (
    id UUID PRIMARY KEY,
    generation BIGINT NOT NULL DEFAULT 0
);
INSERT INTO passkey_accounts(id) VALUES ('00000000-0000-0000-0000-000000000001');

CREATE TABLE passkey_credentials (
    id UUID PRIMARY KEY,
    account_id UUID NOT NULL REFERENCES passkey_accounts(id) ON DELETE CASCADE,
    credential_id BYTEA NOT NULL UNIQUE CHECK (octet_length(credential_id) BETWEEN 1 AND 1024),
    payload JSONB NOT NULL,
    signature_counter BIGINT NOT NULL DEFAULT 0 CHECK (signature_counter BETWEEN 0 AND 4294967295),
    name TEXT NOT NULL CHECK (octet_length(name) BETWEEN 1 AND 256),
    created_at BIGINT NOT NULL,
    last_used_at BIGINT
);
CREATE INDEX passkey_credentials_account ON passkey_credentials(account_id);

CREATE TABLE passkey_ceremonies (
    id UUID PRIMARY KEY,
    account_id UUID NOT NULL REFERENCES passkey_accounts(id) ON DELETE CASCADE,
    generation BIGINT NOT NULL,
    purpose TEXT NOT NULL,
    binding_hash TEXT NOT NULL,
    authorization_binding TEXT NOT NULL,
    state JSONB NOT NULL,
    expires_at BIGINT NOT NULL
);
CREATE INDEX passkey_ceremonies_account ON passkey_ceremonies(account_id);
CREATE INDEX passkey_ceremonies_expiry ON passkey_ceremonies(expires_at);

CREATE TABLE singbox_portal_accounts (
    user_id BIGINT PRIMARY KEY REFERENCES users(id),
    account_id UUID NOT NULL UNIQUE REFERENCES passkey_accounts(id) ON DELETE CASCADE,
    activation_hash TEXT UNIQUE,
    activation_expires_at BIGINT,
    CHECK ((activation_hash IS NULL) = (activation_expires_at IS NULL))
);

CREATE TABLE singbox_portal_sessions (
    token_hash TEXT PRIMARY KEY,
    account_id UUID NOT NULL REFERENCES passkey_accounts(id) ON DELETE CASCADE,
    expires_at BIGINT NOT NULL,
    verified_at BIGINT NOT NULL
);
CREATE INDEX singbox_portal_sessions_account ON singbox_portal_sessions(account_id);
CREATE INDEX singbox_portal_sessions_expiry ON singbox_portal_sessions(expires_at);
