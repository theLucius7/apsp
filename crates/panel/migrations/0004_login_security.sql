ALTER TABLE admins
    ADD COLUMN totp_secret BYTEA CHECK (totp_secret IS NULL OR octet_length(totp_secret) = 20),
    ADD COLUMN totp_last_step BIGINT CHECK (totp_last_step IS NULL OR totp_last_step >= 0),
    ADD COLUMN totp_pending_secret BYTEA CHECK (totp_pending_secret IS NULL OR octet_length(totp_pending_secret) = 20),
    ADD COLUMN totp_pending_expires BIGINT,
    ADD COLUMN totp_pending_session TEXT;

CREATE TABLE auth_rate_limits (
    scope TEXT PRIMARY KEY,
    window_start BIGINT NOT NULL,
    attempts INTEGER NOT NULL CHECK (attempts >= 0)
);
INSERT INTO auth_rate_limits (scope, window_start, attempts) VALUES ('global', 0, 0);
