ALTER TABLE diagnostic_jobs DROP CONSTRAINT diagnostic_jobs_status_check;
ALTER TABLE diagnostic_jobs ADD CONSTRAINT diagnostic_jobs_status_check
    CHECK (status IN ('queued', 'running', 'cleaning', 'cancel_requested', 'cancelled', 'succeeded', 'failed'));

DROP INDEX diagnostic_active_server_idx;
CREATE UNIQUE INDEX diagnostic_active_server_idx ON diagnostic_jobs(server_id)
    WHERE status IN ('queued', 'running', 'cleaning');
