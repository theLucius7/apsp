-- Legacy targets without an authorization record require explicit confirmation.
-- Keep recorded grants, IDs and history; runtime validation owns expiry, identity
-- and revocation checks rather than duplicating that policy in this migration.
UPDATE network_probes SET spec = jsonb_set(spec, '{enabled}', 'false'::jsonb)
    WHERE spec #> '{monitor,authorization}' IS NULL
       OR spec #> '{monitor,authorization}' = 'null'::jsonb;
UPDATE latency_tasks SET spec = jsonb_set(spec, '{enabled}', 'false'::jsonb),
    default_enabled = FALSE, revision = revision + 1
    WHERE spec #> '{monitor,authorization}' IS NULL
       OR spec #> '{monitor,authorization}' = 'null'::jsonb;
