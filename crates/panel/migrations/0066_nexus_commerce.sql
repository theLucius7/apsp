-- Native member commerce belongs to the proxy business plugin.
ALTER TABLE singbox_user_policies ADD COLUMN manual_grant BOOLEAN NOT NULL DEFAULT TRUE;
ALTER TABLE singbox_user_policies ADD COLUMN commerce_grant BOOLEAN NOT NULL DEFAULT FALSE;
CREATE TABLE singbox_commerce_settings (
    singleton BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK(singleton),
    enabled BOOLEAN NOT NULL DEFAULT FALSE,
    invitations_enabled BOOLEAN NOT NULL DEFAULT FALSE,
    invite_price_cents BIGINT NOT NULL DEFAULT 100 CHECK(invite_price_cents BETWEEN 1 AND 100000000),
    version BIGINT NOT NULL DEFAULT 1,
    updated_at BIGINT NOT NULL DEFAULT 0
);
INSERT INTO singbox_commerce_settings(singleton) VALUES(TRUE);

CREATE TABLE singbox_members (
    user_id BIGINT PRIMARY KEY REFERENCES users(id),
    email TEXT UNIQUE,
    state TEXT NOT NULL DEFAULT 'active' CHECK(state IN('active','pending','suspended','archived','staged')),
    inviter_id BIGINT REFERENCES users(id),
    sponsor_required BOOLEAN NOT NULL DEFAULT FALSE,
    first_purchase_at BIGINT,
    version BIGINT NOT NULL DEFAULT 1,
    notes TEXT NOT NULL DEFAULT '',
    created_at BIGINT NOT NULL,
    CHECK(inviter_id IS NULL OR inviter_id <> user_id)
);
CREATE TABLE singbox_wallets (
    user_id BIGINT PRIMARY KEY REFERENCES users(id),
    balance_cents BIGINT NOT NULL DEFAULT 0 CHECK(balance_cents BETWEEN 0 AND 2147483647)
);
CREATE TABLE singbox_wallet_ledger (
    id BIGSERIAL PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES users(id),
    request_key TEXT NOT NULL,
    request_hash TEXT NOT NULL,
    action TEXT NOT NULL,
    delta_cents BIGINT NOT NULL,
    balance_before BIGINT NOT NULL CHECK(balance_before BETWEEN 0 AND 2147483647),
    balance_after BIGINT NOT NULL CHECK(balance_after BETWEEN 0 AND 2147483647),
    source_id TEXT NOT NULL,
    actor TEXT NOT NULL,
    reason TEXT NOT NULL,
    created_at BIGINT NOT NULL,
    UNIQUE(user_id,request_key),
    CHECK(balance_after=balance_before+delta_cents)
);
CREATE INDEX singbox_wallet_ledger_member ON singbox_wallet_ledger(user_id,id DESC);

CREATE TABLE singbox_commerce_offers (
    id BIGSERIAL PRIMARY KEY,
    name TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    package_group_id BIGINT NOT NULL REFERENCES singbox_package_groups(id),
    package_snapshot JSONB NOT NULL,
    policy_group_ids BIGINT[] NOT NULL DEFAULT '{}',
    price_cents BIGINT NOT NULL CHECK(price_cents BETWEEN 0 AND 100000000),
    duration_months INTEGER CHECK(duration_months BETWEEN 1 AND 1200),
    enabled BOOLEAN NOT NULL DEFAULT FALSE,
    renewable BOOLEAN NOT NULL DEFAULT TRUE,
    capacity INTEGER CHECK(capacity BETWEEN 1 AND 1000000),
    version BIGINT NOT NULL DEFAULT 1,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL
);
CREATE TABLE singbox_commerce_orders (
    id UUID PRIMARY KEY,
    payer_id BIGINT NOT NULL REFERENCES users(id),
    beneficiary_id BIGINT NOT NULL REFERENCES users(id),
    request_id UUID NOT NULL,
    request_hash TEXT NOT NULL,
    offer_id BIGINT NOT NULL REFERENCES singbox_commerce_offers(id),
    snapshot JSONB NOT NULL,
    amount_cents BIGINT NOT NULL CHECK(amount_cents BETWEEN 0 AND 100000000),
    assignment_id BIGINT REFERENCES singbox_package_assignments(id),
    state TEXT NOT NULL CHECK(state IN('fulfilled','refunded')),
    created_at BIGINT NOT NULL,
    UNIQUE(payer_id,request_id)
);
CREATE INDEX singbox_commerce_orders_member ON singbox_commerce_orders(beneficiary_id,created_at DESC);

CREATE TABLE singbox_commerce_invitations (
    id UUID PRIMARY KEY,
    issuer_id BIGINT NOT NULL REFERENCES users(id),
    request_id UUID NOT NULL,
    request_hash TEXT NOT NULL,
    token_hash TEXT NOT NULL UNIQUE,
    offer_id BIGINT REFERENCES singbox_commerce_offers(id),
    snapshot JSONB,
    admission_cents BIGINT NOT NULL CHECK(admission_cents > 0),
    prepaid_cents BIGINT NOT NULL DEFAULT 0 CHECK(prepaid_cents >= 0),
    funding_ledger_id BIGINT NOT NULL REFERENCES singbox_wallet_ledger(id),
    batch_id UUID NOT NULL,
    state TEXT NOT NULL DEFAULT 'available' CHECK(state IN('available','claimed','revoked')),
    recipient_id BIGINT UNIQUE REFERENCES users(id),
    created_at BIGINT NOT NULL,
    claim_hash TEXT,
    claimed_at BIGINT,
    revoked_at BIGINT,
    UNIQUE(issuer_id,request_id),
    CHECK((offer_id IS NULL)=(snapshot IS NULL)),
    CHECK((state='claimed')=(recipient_id IS NOT NULL))
);
CREATE INDEX singbox_commerce_invitations_issuer ON singbox_commerce_invitations(issuer_id,created_at DESC);
CREATE TABLE singbox_commerce_events (
    id BIGSERIAL PRIMARY KEY,
    user_id BIGINT REFERENCES users(id),
    actor TEXT NOT NULL,
    action TEXT NOT NULL,
    object_id TEXT NOT NULL,
    detail JSONB NOT NULL,
    created_at BIGINT NOT NULL
);

CREATE TABLE singbox_payment_channels (
    id BIGSERIAL PRIMARY KEY,
    name TEXT NOT NULL,
    gateway TEXT NOT NULL,
    merchant TEXT NOT NULL,
    credential_id UUID NOT NULL REFERENCES credential_entries(id),
    payment_type TEXT NOT NULL CHECK(payment_type IN('alipay','wxpay','qqpay')),
    fixed_fee_cents BIGINT NOT NULL DEFAULT 0 CHECK(fixed_fee_cents BETWEEN 0 AND 1000000),
    fee_basis_points INTEGER NOT NULL DEFAULT 0 CHECK(fee_basis_points BETWEEN 0 AND 10000),
    enabled BOOLEAN NOT NULL DEFAULT FALSE,
    version BIGINT NOT NULL DEFAULT 1,
    updated_at BIGINT NOT NULL
);
CREATE TABLE singbox_payment_topups (
    id UUID PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES users(id),
    request_id UUID NOT NULL,
    request_hash TEXT NOT NULL,
    channel_id BIGINT NOT NULL REFERENCES singbox_payment_channels(id),
    merchant TEXT NOT NULL,
    amount_cents BIGINT NOT NULL CHECK(amount_cents BETWEEN 100 AND 1000000),
    fee_cents BIGINT NOT NULL CHECK(fee_cents >= 0),
    state TEXT NOT NULL DEFAULT 'pending' CHECK(state IN('pending','paid','cancelled','expired','exception')),
    created_at BIGINT NOT NULL,
    expires_at BIGINT NOT NULL,
    paid_at BIGINT,
    UNIQUE(user_id,request_id)
);
CREATE TABLE singbox_payment_receipts (
    id BIGSERIAL PRIMARY KEY,
    topup_id UUID NOT NULL REFERENCES singbox_payment_topups(id),
    channel_id BIGINT NOT NULL REFERENCES singbox_payment_channels(id),
    provider_reference TEXT NOT NULL,
    amount_cents BIGINT NOT NULL,
    created_at BIGINT NOT NULL,
    UNIQUE(channel_id,provider_reference)
);
CREATE TABLE singbox_payment_exceptions (
    id BIGSERIAL PRIMARY KEY,
    topup_id UUID NOT NULL REFERENCES singbox_payment_topups(id),
    receipt_id BIGINT NOT NULL UNIQUE REFERENCES singbox_payment_receipts(id),
    code TEXT NOT NULL,
    state TEXT NOT NULL DEFAULT 'open' CHECK(state IN('open','credited','refunded')),
    resolution TEXT,
    created_at BIGINT NOT NULL,
    resolved_at BIGINT
);

CREATE TABLE singbox_member_content (
    id BIGSERIAL PRIMARY KEY,
    kind TEXT NOT NULL CHECK(kind IN('notice','guide')),
    title TEXT NOT NULL,
    category TEXT NOT NULL DEFAULT '',
    body TEXT NOT NULL,
    published BOOLEAN NOT NULL DEFAULT FALSE,
    version BIGINT NOT NULL DEFAULT 1,
    updated_at BIGINT NOT NULL
);
CREATE TABLE singbox_nexus_imports (
    id UUID PRIMARY KEY,
    source_id TEXT NOT NULL UNIQUE,
    digest TEXT NOT NULL,
    counts JSONB NOT NULL,
    imported_at BIGINT NOT NULL,
    administrator_id BIGINT NOT NULL
);
CREATE TABLE singbox_nexus_identities (
    user_id BIGINT PRIMARY KEY REFERENCES users(id),
    import_id UUID NOT NULL REFERENCES singbox_nexus_imports(id),
    legacy_uuid UUID NOT NULL,
    original_state TEXT NOT NULL,
    original_plan_id BIGINT,
    quota_bytes NUMERIC(30,0),
    used_upload NUMERIC(30,0) NOT NULL,
    used_download NUMERIC(30,0) NOT NULL,
    expires_at BIGINT,
    reset_at BIGINT
);

CREATE TABLE singbox_nexus_records (
    import_id UUID NOT NULL REFERENCES singbox_nexus_imports(id),
    kind TEXT NOT NULL,
    source_id TEXT NOT NULL,
    user_id BIGINT REFERENCES users(id),
    record JSONB NOT NULL,
    PRIMARY KEY(import_id,kind,source_id)
);

-- Existing users without a member profile retain their original eligibility.
CREATE OR REPLACE FUNCTION singbox_entitlements(at_s BIGINT)
RETURNS TABLE (user_id BIGINT, package_group_id BIGINT, package_name TEXT, monthly_bytes TEXT,
    reset_day INTEGER, reset_hour INTEGER, reset_minute INTEGER, timezone TEXT,
    starts_at BIGINT, expires_at BIGINT, cycle_start BIGINT, next_reset BIGINT,
    used_bytes TEXT, status TEXT, allowed BOOLEAN)
LANGUAGE sql STABLE AS $$
    WITH evaluated AS (
        SELECT u.id, a.package_group_id, a.package_name, a.monthly_bytes,
            a.reset_day, a.reset_hour, a.reset_minute, a.timezone, a.starts_at, a.expires_at,
            b.cycle_start, b.next_reset,
            GREATEST(COALESCE(t.used,0)-COALESCE(c.credit,0),0) AS used,
            CASE WHEN m.state IS NOT NULL AND m.state <> 'active' THEN m.state
                WHEN a.id IS NULL THEN 'unmetered'
                WHEN at_s < a.starts_at THEN 'not_started'
                WHEN at_s >= a.expires_at THEN 'expired'
                WHEN a.monthly_bytes IS NOT NULL AND GREATEST(COALESCE(t.used,0)-COALESCE(c.credit,0),0)>=a.monthly_bytes THEN 'exhausted'
                ELSE 'active' END AS status
        FROM users u
        LEFT JOIN singbox_members m ON m.user_id=u.id
        LEFT JOIN singbox_user_packages p ON p.user_id=u.id
        LEFT JOIN singbox_package_assignments a ON a.id=p.assignment_id
        LEFT JOIN LATERAL singbox_cycle_bounds(at_s,a.reset_day,a.reset_hour,a.reset_minute,a.timezone) b ON TRUE
        LEFT JOIN LATERAL (SELECT singbox_usage_window(u.id,b.cycle_start,b.next_reset) AS used) t ON TRUE
        LEFT JOIN LATERAL (SELECT SUM(q.bytes) AS credit FROM singbox_quota_credits q WHERE q.user_id=u.id AND q.cycle_start=b.cycle_start AND q.next_reset=b.next_reset) c ON TRUE
        WHERE u.deleted_at IS NULL
    )
    SELECT id,package_group_id,package_name,monthly_bytes::TEXT,reset_day,reset_hour,reset_minute,timezone,
        starts_at,expires_at,cycle_start,next_reset,used::TEXT,status,status IN('unmetered','active') FROM evaluated;
$$;
