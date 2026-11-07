ALTER TABLE ddns_rules ADD COLUMN access_key_id TEXT NOT NULL DEFAULT '';
ALTER TABLE ddns_rules ADD COLUMN access_key_secret TEXT NOT NULL DEFAULT '';
DROP INDEX ddns_record_identity;
CREATE UNIQUE INDEX ddns_record_identity ON ddns_rules
    ((COALESCE(config->>'provider','cloudflare')), (config->>'zone_id'),
     (config->>'record_name'), (config->>'record_type'), (COALESCE(config->>'line','')));
