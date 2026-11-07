ALTER TABLE runtime_control_requests DROP CONSTRAINT runtime_control_requests_kind_check;
ALTER TABLE runtime_control_requests ADD CONSTRAINT runtime_control_requests_kind_check
    CHECK (kind IN ('checkpoint', 'barrier', 'probe'));
