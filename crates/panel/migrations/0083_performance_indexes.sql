-- Indexes for queries that run every few seconds (2026-11-04 performance pass).

-- usage_records_user_period_idx (user_id, period_end) already serves every
-- lookup by user_id, including the foreign-key checks from users. The
-- single-column index only added a write to every ledger insert.
DROP INDEX IF EXISTS usage_records_user_idx;

-- Heartbeats replay pending requests and expired barriers. Pending requests
-- already have runtime_control_replay_idx; this covers the second branch so the
-- lookup no longer scans every request the server ever received.
CREATE INDEX runtime_control_expired_barrier_idx
    ON runtime_control_requests(server_id, created_at)
    WHERE kind = 'barrier' AND state = 'expired';

-- Compaction selects one resolution over a time range every minute; the age
-- index alone visits rows of every resolution in that range.
CREATE INDEX telemetry_history_compaction_idx
    ON telemetry_history(resolution_secs, bucket_at, server_id);

-- Agents ask for open terminal sessions every few seconds.
CREATE INDEX fleet_terminal_sessions_open_idx
    ON fleet_terminal_sessions(server_id)
    WHERE status IN ('queued', 'running');
