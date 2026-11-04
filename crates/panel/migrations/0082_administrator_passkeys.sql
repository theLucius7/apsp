-- Every administrator signs in with their own passkeys (ADR 0107). The initial
-- owner keeps the account created in 0039, so keys bound before stay valid.
-- Other administrators get an account on their first registration.
CREATE TABLE administrator_passkey_accounts (
    admin_id BIGINT PRIMARY KEY REFERENCES admins(id) ON DELETE CASCADE,
    account_id UUID NOT NULL UNIQUE REFERENCES passkey_accounts(id) ON DELETE CASCADE
);
INSERT INTO administrator_passkey_accounts(admin_id, account_id)
SELECT id, '00000000-0000-0000-0000-000000000001' FROM admins WHERE id = 1;
