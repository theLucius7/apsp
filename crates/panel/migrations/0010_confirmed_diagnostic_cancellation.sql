ALTER TABLE diagnostic_jobs
    ADD COLUMN cancel_requested_at BIGINT,
    ADD COLUMN cancel_confirmed_at BIGINT,
    ADD COLUMN cancel_error TEXT;

ALTER TABLE diagnostic_jobs DROP CONSTRAINT diagnostic_jobs_status_check;
ALTER TABLE diagnostic_jobs ADD CONSTRAINT diagnostic_jobs_status_check
    CHECK (status IN ('queued', 'running', 'cancel_requested', 'cancelled', 'succeeded', 'failed'));

CREATE INDEX diagnostic_cancel_pending_idx ON diagnostic_jobs(server_id, cancel_requested_at)
    WHERE status = 'cancel_requested';
