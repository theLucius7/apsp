-- Invitations no longer cost money (ADR 0113). Each proxy user has a number of
-- invitations they may still hand out; each code creates one account. Balances
-- left from the paid model are converted at the last price of one invitation.

CREATE TABLE singbox_invitation_settings (
    singleton BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK(singleton),
    enabled BOOLEAN NOT NULL DEFAULT FALSE,
    package_group_id BIGINT REFERENCES singbox_package_groups(id),
    default_quota INTEGER NOT NULL DEFAULT 0 CHECK(default_quota BETWEEN 0 AND 1000),
    version BIGINT NOT NULL DEFAULT 1,
    updated_at BIGINT NOT NULL DEFAULT 0
);
-- Policy groups every invited account starts with.
CREATE TABLE singbox_invitation_policy_groups (
    group_id BIGINT PRIMARY KEY REFERENCES singbox_policy_groups(id) ON DELETE CASCADE
);

CREATE TABLE singbox_invitation_quotas (
    user_id BIGINT PRIMARY KEY REFERENCES users(id),
    remaining INTEGER NOT NULL DEFAULT 0 CHECK(remaining BETWEEN 0 AND 100000)
);
CREATE TABLE singbox_invitation_quota_events (
    id BIGSERIAL PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES users(id),
    delta INTEGER NOT NULL,
    remaining INTEGER NOT NULL,
    reason TEXT NOT NULL CHECK(reason IN('balance_conversion','administrator','invitation_created','invitation_revoked','invited_account')),
    actor TEXT NOT NULL,
    detail JSONB NOT NULL DEFAULT '{}'::JSONB,
    created_at BIGINT NOT NULL
);
CREATE INDEX singbox_invitation_quota_events_user ON singbox_invitation_quota_events(user_id,id DESC);

CREATE TABLE singbox_invitations (
    id UUID PRIMARY KEY,
    issuer_id BIGINT NOT NULL REFERENCES users(id),
    token_hash TEXT NOT NULL UNIQUE,
    -- The last characters of the code, so the issuer can tell codes apart.
    hint TEXT NOT NULL DEFAULT '',
    origin TEXT NOT NULL DEFAULT 'quota' CHECK(origin IN('quota','paid')),
    -- Set on the first code of a batch, so a retried request is not charged twice.
    request_id UUID,
    state TEXT NOT NULL DEFAULT 'available' CHECK(state IN('available','claimed','revoked')),
    recipient_id BIGINT UNIQUE REFERENCES users(id),
    claim_hash TEXT,
    created_at BIGINT NOT NULL,
    claimed_at BIGINT,
    revoked_at BIGINT,
    CHECK((state='claimed')=(recipient_id IS NOT NULL))
);
CREATE INDEX singbox_invitations_issuer ON singbox_invitations(issuer_id,created_at DESC);
CREATE UNIQUE INDEX singbox_invitations_request ON singbox_invitations(issuer_id,request_id) WHERE request_id IS NOT NULL;

INSERT INTO singbox_invitation_settings(enabled,updated_at)
    SELECT enabled AND invitations_enabled,EXTRACT(EPOCH FROM now())::BIGINT FROM singbox_commerce_settings WHERE singleton;
INSERT INTO singbox_invitation_settings(singleton) VALUES(TRUE) ON CONFLICT DO NOTHING;

-- Paid invitations keep their links and history. One that was never claimed stays
-- usable; the package price prepaid with it is refunded before the conversion.
INSERT INTO singbox_invitations(id,issuer_id,token_hash,origin,state,recipient_id,claim_hash,created_at,claimed_at,revoked_at)
    SELECT id,issuer_id,token_hash,'paid',state,recipient_id,claim_hash,created_at,claimed_at,revoked_at
    FROM singbox_commerce_invitations;

DO $$
DECLARE
    price BIGINT;
    item RECORD;
    now_s BIGINT := EXTRACT(EPOCH FROM now())::BIGINT;
    total BIGINT;
    granted INTEGER;
BEGIN
    SELECT invite_price_cents INTO price FROM singbox_commerce_settings WHERE singleton;
    price := GREATEST(COALESCE(price,100),1);
    FOR item IN
        SELECT w.user_id,w.balance_cents,COALESCE((
            SELECT sum(i.prepaid_cents) FROM singbox_commerce_invitations i
            WHERE i.issuer_id=w.user_id AND i.state='available' AND i.offer_id IS NOT NULL),0) AS prepaid
        FROM singbox_wallets w JOIN users u ON u.id=w.user_id
        ORDER BY w.user_id
    LOOP
        total := item.balance_cents+item.prepaid;
        IF total<=0 THEN
            CONTINUE;
        END IF;
        granted := LEAST(total/price,100000)::INTEGER;
        IF item.prepaid>0 THEN
            INSERT INTO singbox_wallet_ledger(user_id,request_key,request_hash,action,delta_cents,balance_before,balance_after,source_id,actor,reason,created_at)
                VALUES(item.user_id,'migration-0087:prepaid','migration-0087','invite_refund',item.prepaid,item.balance_cents,LEAST(total,2147483647),'migration-0087','system','未领取预付邀请的套餐款退回，随后换算为邀请名额',now_s);
        END IF;
        INSERT INTO singbox_wallet_ledger(user_id,request_key,request_hash,action,delta_cents,balance_before,balance_after,source_id,actor,reason,created_at)
            VALUES(item.user_id,'migration-0087:conversion','migration-0087','invite_quota_conversion',-LEAST(total,2147483647),LEAST(total,2147483647),0,'migration-0087','system',
                format('余额按每份邀请 %s 元换算为 %s 个邀请名额', to_char(price/100.0,'FM999999990.00'), granted),now_s);
        UPDATE singbox_wallets SET balance_cents=0 WHERE user_id=item.user_id;
        INSERT INTO singbox_invitation_quotas(user_id,remaining) VALUES(item.user_id,granted)
            ON CONFLICT(user_id) DO UPDATE SET remaining=LEAST(singbox_invitation_quotas.remaining+EXCLUDED.remaining,100000);
        INSERT INTO singbox_invitation_quota_events(user_id,delta,remaining,reason,actor,detail,created_at)
            SELECT item.user_id,granted,q.remaining,'balance_conversion','system',
                jsonb_build_object('balance_cents',item.balance_cents,'prepaid_refund_cents',item.prepaid,'price_cents',price,'remainder_cents',total-granted::BIGINT*price),now_s
            FROM singbox_invitation_quotas q WHERE q.user_id=item.user_id;
    END LOOP;
END $$;

-- Nobody buys a first package for an invited member any more: an account that was
-- waiting for that becomes an ordinary member. Without a package it still cannot
-- use the service until an administrator assigns one.
UPDATE singbox_members SET state='active',sponsor_required=FALSE,version=version+1
    WHERE sponsor_required AND first_purchase_at IS NULL AND state IN('active','pending');
