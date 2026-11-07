-- Mixed chains become ordered chains one at a time, only when an administrator
-- converts them (ADR 0085 phase 3, step S1d). The mixed active generation keeps
-- routing while the ordered candidate is prepared and probed; a failure before
-- the entry switches returns the chain to mixed. After the switch the old
-- mixed generation stays on its dependencies until the ordered recovery
-- barrier, then it is retired.
CREATE TABLE singbox_mixed_conversions (
    chain_id BIGINT PRIMARY KEY REFERENCES singbox_chains(id),
    mixed_generation BIGINT NOT NULL,
    ordered_generation BIGINT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('preparing','switched','completed','reverted')),
    attempts INTEGER NOT NULL DEFAULT 1 CHECK (attempts > 0),
    started_at BIGINT NOT NULL,
    switched_at BIGINT,
    finished_at BIGINT,
    last_error TEXT,
    FOREIGN KEY (chain_id, mixed_generation)
        REFERENCES singbox_chain_versions(chain_id, generation),
    FOREIGN KEY (chain_id, ordered_generation)
        REFERENCES singbox_ordered_chain_versions(chain_id, generation)
);

-- A device keeps every committed recovery floor of a mixed path scope and
-- rejects any later bundle whose runtime constraints omit it. Once a converted
-- chain no longer compiles as mixed, these rows carry the floors. They outlive
-- the mixed tables and are never deleted.
CREATE TABLE singbox_retired_path_scopes (
    server_id BIGINT NOT NULL REFERENCES servers(id),
    scope TEXT NOT NULL,
    floor BIGINT NOT NULL CHECK (floor > 0),
    chain_id BIGINT NOT NULL REFERENCES singbox_chains(id),
    recorded_at BIGINT NOT NULL,
    PRIMARY KEY (server_id, scope)
);
