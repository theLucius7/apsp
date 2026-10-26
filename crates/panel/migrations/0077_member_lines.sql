-- Member line catalogue and probe panel (ADR 0101). While a Nexus node is
-- still served by the old node software, its online state, load and carrier
-- latency come from the legacy panel through the compatibility bridge. No
-- endpoint address, monitor identifier or member data is stored here.

CREATE TABLE singbox_nexus_node_status (
    legacy_node_id BIGINT PRIMARY KEY CHECK (legacy_node_id > 0),
    online BOOLEAN NOT NULL,
    checked_at BIGINT,
    load JSONB NOT NULL DEFAULT '{}'::JSONB CHECK (jsonb_typeof(load) = 'object'),
    latency JSONB NOT NULL DEFAULT '{}'::JSONB CHECK (jsonb_typeof(latency) = 'object'),
    observed_at BIGINT NOT NULL
);

CREATE FUNCTION singbox_nexus_node_status_exchange(sample JSONB) RETURNS JSONB
LANGUAGE plpgsql AS $$
DECLARE bridge singbox_nexus_bridge%ROWTYPE; observed BIGINT; now_s BIGINT; entry JSONB; carrier TEXT; measured JSONB; key TEXT; ids BIGINT[] := '{}';
BEGIN
    PERFORM pg_advisory_xact_lock(73201003);
    SELECT * INTO STRICT bridge FROM singbox_nexus_bridge WHERE singleton AND enabled;
    IF sample->>'import_id' IS DISTINCT FROM bridge.import_id::TEXT
        OR jsonb_typeof(sample->'nodes') IS DISTINCT FROM 'array'
        OR jsonb_array_length(sample->'nodes') > 5000
        OR COALESCE(sample->>'observed_at','') !~ '^[0-9]{1,12}$' THEN
        RAISE EXCEPTION 'Invalid node status sample';
    END IF;
    now_s = EXTRACT(EPOCH FROM now())::BIGINT;
    observed = (sample->>'observed_at')::BIGINT;
    IF observed < now_s-120 OR observed > now_s+30 THEN RAISE EXCEPTION 'Expired node status sample'; END IF;
    FOR entry IN SELECT value FROM jsonb_array_elements(sample->'nodes') LOOP
        IF COALESCE(entry->>'id','') !~ '^[1-9][0-9]{0,15}$' OR jsonb_typeof(entry->'online') <> 'boolean'
            OR (entry ? 'checked_at' AND jsonb_typeof(entry->'checked_at') NOT IN ('number','null'))
            OR jsonb_typeof(COALESCE(entry->'load','{}')) <> 'object'
            OR jsonb_typeof(COALESCE(entry->'latency','{}')) <> 'object'
            OR jsonb_typeof(COALESCE(entry->'latency'->'carriers','{}')) <> 'object' THEN
            RAISE EXCEPTION 'Invalid node status';
        END IF;
        -- Only numeric load percentages and the three carrier measurements.
        FOR key IN SELECT jsonb_object_keys(COALESCE(entry->'load','{}')) LOOP
            IF key NOT IN ('cpu','memory','disk') OR jsonb_typeof(entry->'load'->key) NOT IN ('number','null') THEN
                RAISE EXCEPTION 'Invalid node load';
            END IF;
        END LOOP;
        FOR carrier, measured IN SELECT * FROM jsonb_each(COALESCE(entry->'latency'->'carriers','{}')) LOOP
            IF carrier NOT IN ('telecom','unicom','mobile') OR jsonb_typeof(measured) <> 'object'
                OR EXISTS(SELECT 1 FROM jsonb_each(measured) m WHERE m.key NOT IN ('latest','average','loss') OR jsonb_typeof(m.value) NOT IN ('number','null')) THEN
                RAISE EXCEPTION 'Invalid node latency';
            END IF;
        END LOOP;
        ids = ids || (entry->>'id')::BIGINT;
        INSERT INTO singbox_nexus_node_status(legacy_node_id,online,checked_at,load,latency,observed_at)
        VALUES((entry->>'id')::BIGINT, (entry->>'online')::BOOLEAN, NULLIF(entry->>'checked_at','')::BIGINT,
               COALESCE(entry->'load','{}'),
               jsonb_build_object('monitored', COALESCE((entry->'latency'->>'monitored')::BOOLEAN,false),
                   'updated_at', CASE WHEN jsonb_typeof(entry->'latency'->'updated_at')='number' THEN entry->'latency'->'updated_at' ELSE 'null'::JSONB END,
                   'carriers', COALESCE(entry->'latency'->'carriers','{}')),
               observed)
        ON CONFLICT(legacy_node_id) DO UPDATE SET online=EXCLUDED.online, checked_at=EXCLUDED.checked_at,
            load=EXCLUDED.load, latency=EXCLUDED.latency, observed_at=EXCLUDED.observed_at
            WHERE EXCLUDED.observed_at>=singbox_nexus_node_status.observed_at;
    END LOOP;
    -- Nodes the legacy panel no longer lists have no current status.
    DELETE FROM singbox_nexus_node_status WHERE legacy_node_id <> ALL(ids);
    RETURN jsonb_build_object('nodes', cardinality(ids));
END;
$$;
