-- Keep earlier migration bytes unchanged so existing migration checksums remain valid.
ALTER FUNCTION sinan_traffic_cycle_start(BIGINT, INTEGER) RENAME TO apsp_traffic_cycle_start;

ALTER TABLE singbox_subscription_sources
    ALTER COLUMN user_agent SET DEFAULT 'Apsp-subscription-import/1';
UPDATE singbox_subscription_sources
SET user_agent = 'Apsp-subscription-import/1'
WHERE user_agent = 'Sinan-subscription-import/1';

DROP INDEX network_managed_listener;
UPDATE network_documents
SET config = jsonb_set(config, '{owner}', '"apsp"'::jsonb)
WHERE kind = 'forwarding' AND config->>'owner' = 'sinan';
CREATE UNIQUE INDEX network_managed_listener
ON network_documents((config->>'server_id'), (config->>'listen_address'),
                     (config->>'listen_port'), (config->>'protocol'))
WHERE kind = 'forwarding' AND config->>'owner' = 'apsp' AND config->>'enabled' = 'true';
