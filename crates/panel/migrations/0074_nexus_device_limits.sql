-- Concurrent client address limits for Nexus-compatible members (ADR 0098).
-- Client addresses are personal data: only addresses of currently admitted
-- sessions are kept, for minutes, and the Agent never reports destinations.

-- Loopback session control secret, fixed per host so candidate digests stay
-- stable; gen_random_uuid() draws from the server's strong random source.
ALTER TABLE singbox_nexus_runtime_hosts ADD COLUMN session_secret TEXT NOT NULL
    DEFAULT (replace(gen_random_uuid()::TEXT,'-','') || replace(gen_random_uuid()::TEXT,'-',''))
    CHECK (session_secret ~ '^[0-9a-f]{64}$');

CREATE TABLE singbox_runtime_session_addresses (
    server_id BIGINT NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
    stat_name TEXT NOT NULL,
    user_id BIGINT NOT NULL,
    node_id BIGINT NOT NULL,
    address TEXT NOT NULL,
    observed_at BIGINT NOT NULL,
    PRIMARY KEY (server_id, stat_name, address)
);
CREATE INDEX singbox_runtime_session_addresses_user
    ON singbox_runtime_session_addresses(user_id, observed_at);

-- The legacy panel's per-member device limit and the addresses its own nodes
-- currently see, delivered by the compatibility bridge.
CREATE TABLE singbox_nexus_device_state (
    user_id BIGINT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    device_limit INTEGER NOT NULL CHECK (device_limit BETWEEN 0 AND 1024),
    legacy_addresses JSONB NOT NULL DEFAULT '[]'::JSONB
        CHECK (jsonb_typeof(legacy_addresses) = 'array'),
    observed_at BIGINT NOT NULL
);

CREATE FUNCTION singbox_nexus_device_exchange(sample JSONB) RETURNS JSONB
LANGUAGE plpgsql AS $$
DECLARE bridge singbox_nexus_bridge%ROWTYPE; observed BIGINT; now_s BIGINT; key TEXT; value JSONB; result JSONB;
BEGIN
    PERFORM pg_advisory_xact_lock(73201002);
    SELECT * INTO STRICT bridge FROM singbox_nexus_bridge WHERE singleton AND enabled;
    IF sample->>'import_id' IS DISTINCT FROM bridge.import_id::TEXT
        OR jsonb_typeof(sample->'limits') IS DISTINCT FROM 'object'
        OR jsonb_typeof(sample->'legacy') IS DISTINCT FROM 'object'
        OR COALESCE(sample->>'observed_at','') !~ '^[0-9]{1,12}$' THEN
        RAISE EXCEPTION 'Invalid device sample';
    END IF;
    now_s = EXTRACT(EPOCH FROM now())::BIGINT;
    observed = (sample->>'observed_at')::BIGINT;
    IF observed < now_s-120 OR observed > now_s+30 THEN RAISE EXCEPTION 'Expired device sample'; END IF;
    IF (SELECT count(*) FROM jsonb_object_keys(sample->'limits')) > 20000
        OR (SELECT count(*) FROM jsonb_object_keys(sample->'legacy')) > 20000 THEN
        RAISE EXCEPTION 'Invalid device sample';
    END IF;
    FOR key, value IN SELECT * FROM jsonb_each(sample->'limits') LOOP
        IF key !~ '^[1-9][0-9]{0,15}$' OR jsonb_typeof(value) <> 'number'
            OR value::TEXT !~ '^[0-9]{1,4}$' OR value::TEXT::INTEGER > 1024 THEN
            RAISE EXCEPTION 'Invalid device limit';
        END IF;
    END LOOP;
    FOR key, value IN SELECT * FROM jsonb_each(sample->'legacy') LOOP
        IF key !~ '^[1-9][0-9]{0,15}$' OR jsonb_typeof(value) <> 'array' OR jsonb_array_length(value) > 64
            OR EXISTS(SELECT 1 FROM jsonb_array_elements(value) a WHERE jsonb_typeof(a) <> 'string')
            OR EXISTS(SELECT 1 FROM jsonb_array_elements_text(value) a
                WHERE masklen(a::INET) <> CASE family(a::INET) WHEN 4 THEN 32 ELSE 128 END) THEN
            RAISE EXCEPTION 'Invalid legacy devices';
        END IF;
    END LOOP;
    INSERT INTO singbox_nexus_device_state(user_id,device_limit,legacy_addresses,observed_at)
        SELECT i.user_id, COALESCE((sample->'limits'->>i.user_id::TEXT)::INTEGER,0),
               COALESCE(sample->'legacy'->(i.user_id::TEXT),'[]'::JSONB), observed
        FROM singbox_nexus_identities i WHERE i.import_id=bridge.import_id
    ON CONFLICT(user_id) DO UPDATE SET device_limit=EXCLUDED.device_limit,
        legacy_addresses=EXCLUDED.legacy_addresses, observed_at=EXCLUDED.observed_at
        WHERE EXCLUDED.observed_at>=singbox_nexus_device_state.observed_at;
    -- Admitted addresses on taken-over nodes, keyed by their legacy node so the
    -- legacy panel counts them like its own nodes' devices.
    SELECT COALESCE(jsonb_agg(jsonb_build_object('legacy_node_id',n.legacy_node_id,'users',n.users)
        ORDER BY n.legacy_node_id),'[]'::JSONB) INTO result
    FROM (
        SELECT i.legacy_node_id, COALESCE((
            SELECT jsonb_object_agg(a.user_id::TEXT, a.addresses)
            FROM (SELECT user_id, jsonb_agg(DISTINCT address) AS addresses
                  FROM singbox_runtime_session_addresses
                  WHERE server_id=r.server_id AND node_id=r.node_id AND observed_at>=now_s-60
                  GROUP BY user_id) a), '{}'::JSONB) AS users
        FROM singbox_nexus_runtime_nodes r
        JOIN singbox_nexus_runtime_hosts h ON h.server_id=r.server_id
        JOIN singbox_nexus_nodes i ON i.id=r.inventory_id
        WHERE h.state='active'
    ) n;
    RETURN jsonb_build_object('managed', result);
END;
$$;
