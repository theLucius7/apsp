-- New accounts use only CDT and ECS permissions unless BSS is explicitly enabled.
ALTER TABLE alicloud_accounts
    ADD COLUMN billing_enabled boolean NOT NULL DEFAULT false,
    ADD COLUMN balance_enabled boolean NOT NULL DEFAULT false;

-- Preserve billing evidence for existing automatic policies that depend on it.
UPDATE alicloud_accounts a SET billing_enabled = true
WHERE a.auto_enabled OR EXISTS (
    SELECT 1 FROM alicloud_resources r WHERE r.account_id = a.id AND NOT r.archived
    AND r.power_policy->>'enabled' = 'true'
    AND r.power_policy->>'threshold_action' IN ('notify', 'stop')
);
UPDATE alicloud_accounts SET error_code = NULL WHERE NOT billing_enabled;
UPDATE alicloud_accounts SET balance_error = NULL;
UPDATE alicloud_resources r SET bill_error = NULL
FROM alicloud_accounts a WHERE r.account_id = a.id AND NOT a.billing_enabled;
