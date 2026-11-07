CREATE INDEX IF NOT EXISTS usage_outbox_pending_order_idx
    ON usage_outbox(length(seq), seq)
    WHERE acknowledged = 0 AND octet_length(batch) <= 1048447;
CREATE INDEX IF NOT EXISTS usage_outbox_pending_bytes_idx
    ON usage_outbox(octet_length(batch)) WHERE acknowledged = 0;
