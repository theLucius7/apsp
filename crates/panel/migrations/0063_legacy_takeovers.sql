-- Legacy two-hop chains become ordered chains only through the explicit
-- `sinan-panel legacy-takeover apply` command (ADR 0085 phase 3, step S1c).
-- The takeover keeps generation 1 that 0044 already froze; this table keeps
-- the two-hop columns the ordered model clears, so a rollback can restore them.
CREATE TABLE singbox_legacy_takeovers (
    chain_id BIGINT PRIMARY KEY REFERENCES singbox_chains(id),
    exit_node_id BIGINT NOT NULL REFERENCES nodes(id),
    relay_uuid UUID NOT NULL,
    taken_over_at BIGINT NOT NULL
);
