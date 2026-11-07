ALTER TABLE remote_commands
    ADD COLUMN state TEXT NOT NULL DEFAULT 'queued'
        CHECK (state IN ('queued','claimed','running','cancel_requested','cancelled','succeeded','failed','expired','interrupted')),
    ADD COLUMN lifecycle_version INTEGER NOT NULL DEFAULT 0,
    ADD COLUMN claim_id UUID,
    ADD COLUMN claimed_at BIGINT,
    ADD COLUMN started_at BIGINT,
    ADD COLUMN cancel_requested_at BIGINT,
    ADD COLUMN finished_at BIGINT,
    ADD COLUMN cancel_supported BOOLEAN NOT NULL DEFAULT FALSE;

-- An earlier Agent may already have fetched any unfinished legacy command.
-- Do not claim that cancelling those records can prevent execution.
UPDATE remote_commands SET
    state = COALESCE(result->>'status', 'claimed'),
    finished_at = (result->>'finished_at')::BIGINT;

CREATE INDEX command_pending_state_idx ON remote_commands(server_id, state, requested_at);
