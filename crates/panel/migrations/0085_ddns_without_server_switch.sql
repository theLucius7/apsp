-- DDNS no longer has a per-server switch (ADR 0110): DNS updates run in the
-- panel and read the address the Agent already reports, so a rule's own
-- enabled flag is the only switch.
--
-- A rule on a server whose switch was off never synced. Pause it, so that
-- removing the switch does not start DNS writes nobody turned on.
UPDATE ddns_rules d
SET config = jsonb_set(d.config, '{enabled}', 'false'::jsonb),
    revision = d.revision + 1
WHERE d.config->>'enabled' = 'true'
  AND NOT EXISTS (
      SELECT 1 FROM server_plugins p
      WHERE p.server_id = d.server_id AND p.plugin = 'ddns' AND p.enabled
  );

DELETE FROM server_plugins WHERE plugin = 'ddns';
