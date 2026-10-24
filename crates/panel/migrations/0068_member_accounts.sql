ALTER TABLE singbox_members ADD COLUMN email_verified_at BIGINT;
ALTER TABLE singbox_members ADD COLUMN password_hash TEXT;
ALTER TABLE singbox_members ADD COLUMN remind_expire BOOLEAN NOT NULL DEFAULT TRUE;
ALTER TABLE singbox_members ADD COLUMN remind_traffic BOOLEAN NOT NULL DEFAULT TRUE;
ALTER TABLE singbox_members ADD COLUMN remind_invitation BOOLEAN NOT NULL DEFAULT TRUE;
CREATE TABLE singbox_mail_settings (
    singleton BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK(singleton),
    enabled BOOLEAN NOT NULL DEFAULT FALSE,
    reminders_enabled BOOLEAN NOT NULL DEFAULT FALSE,
    host TEXT NOT NULL DEFAULT '',
    port INTEGER NOT NULL DEFAULT 465 CHECK(port BETWEEN 1 AND 65535),
    security TEXT NOT NULL DEFAULT 'tls' CHECK(security IN('tls','starttls')),
    username TEXT NOT NULL DEFAULT '',
    from_address TEXT NOT NULL DEFAULT '',
    credential_id UUID REFERENCES credential_entries(id),
    blocked_domains TEXT[] NOT NULL DEFAULT '{}',
    version BIGINT NOT NULL DEFAULT 1
);
INSERT INTO singbox_mail_settings(singleton) VALUES(TRUE);
CREATE TABLE singbox_account_challenges (
    id UUID PRIMARY KEY,
    purpose TEXT NOT NULL CHECK(purpose IN('invite','reset','email')),
    user_id BIGINT REFERENCES users(id),
    address TEXT NOT NULL,
    old_address TEXT,
    token_hash TEXT NOT NULL,
    old_token_hash TEXT,
    invitation_hash TEXT,
    attempts INTEGER NOT NULL DEFAULT 0,
    created_at BIGINT NOT NULL,
    expires_at BIGINT NOT NULL,
    consumed_at BIGINT
);
CREATE TABLE singbox_account_limits (
    bucket TEXT PRIMARY KEY,
    starts_at BIGINT NOT NULL,
    attempts INTEGER NOT NULL
);
CREATE TABLE singbox_member_messages (
    id BIGSERIAL PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES users(id),
    event_key TEXT NOT NULL UNIQUE,
    kind TEXT NOT NULL,
    message TEXT NOT NULL,
    created_at BIGINT NOT NULL,
    read_at BIGINT,
    mail_state TEXT NOT NULL DEFAULT 'disabled' CHECK(mail_state IN('disabled','pending','sending','sent','failed','cancelled')),
    mail_attempts INTEGER NOT NULL DEFAULT 0,
    next_attempt_at BIGINT NOT NULL DEFAULT 0,
    lease_until BIGINT
);
CREATE INDEX singbox_member_messages_owner ON singbox_member_messages(user_id,id DESC);
