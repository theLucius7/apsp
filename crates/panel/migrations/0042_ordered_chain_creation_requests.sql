-- Plugin-owned immutable creation receipts contain only request digests and IDs.
-- No foreign keys to resources: deleting a chain must not permit replay creation.
CREATE TABLE singbox_ordered_chain_creation_requests (
    request_id UUID PRIMARY KEY CHECK (request_id <> '00000000-0000-0000-0000-000000000000'),
    request_sha256 TEXT NOT NULL CHECK (request_sha256 ~ '^[0-9a-f]{64}$'),
    receipt JSONB NOT NULL CHECK (jsonb_typeof(receipt) = 'object'),
    created_at BIGINT NOT NULL CHECK (created_at >= 0)
);
