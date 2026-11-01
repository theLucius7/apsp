-- Per-user subscription hostnames, see ADR 0103. Everything here is optional: with no
-- row in singbox_subscription_domain the panel keeps serving /sub/{token} as before.
CREATE TABLE singbox_subscription_domain (
    singleton BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (singleton),
    enabled BOOLEAN NOT NULL,
    credential_id UUID NOT NULL REFERENCES credential_entries(id),
    zone_id TEXT NOT NULL,
    zone_name TEXT NOT NULL,
    host_prefix TEXT NOT NULL,
    target TEXT NOT NULL,
    legacy_paths BOOLEAN NOT NULL,
    revision BIGINT NOT NULL DEFAULT 1,
    updated_at BIGINT NOT NULL
);

CREATE TABLE singbox_subscription_hosts (
    user_id BIGINT PRIMARY KEY REFERENCES users(id),
    slug TEXT NOT NULL UNIQUE,
    path_token TEXT NOT NULL UNIQUE,
    record_id TEXT,
    state TEXT NOT NULL DEFAULT 'pending' CHECK (state IN ('pending', 'ready', 'failed')),
    last_error TEXT,
    updated_at BIGINT NOT NULL
);

-- DNS records of rotated or removed hosts, deleted by the worker.
CREATE TABLE singbox_subscription_host_retired (
    record_id TEXT PRIMARY KEY,
    zone_id TEXT NOT NULL,
    retired_at BIGINT NOT NULL
);
