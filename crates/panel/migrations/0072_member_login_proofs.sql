-- A portal session records how it was established. Only a passkey proof may
-- manage existing passkeys; a password proof may change the password and
-- contact address. Existing sessions are treated as password proofs, which at
-- most asks an already logged-in member to verify with a passkey again.
ALTER TABLE singbox_portal_sessions
    ADD COLUMN proof TEXT NOT NULL DEFAULT 'password' CHECK (proof IN ('passkey', 'password'));

-- Password login requires a verified address (ADR 0091). Imported Nexus members
-- keep their original login address (ADR 0092) until it is changed.
ALTER TABLE singbox_members ADD COLUMN legacy_login_email TEXT;
UPDATE singbox_members m
SET legacy_login_email = m.email
FROM singbox_nexus_identities i
WHERE i.user_id = m.user_id
  AND m.password_hash IS NOT NULL
  AND m.email_verified_at IS NULL
  AND m.email IS NOT NULL;
