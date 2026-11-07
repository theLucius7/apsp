ALTER TABLE servers ADD COLUMN asset_settings JSONB NOT NULL DEFAULT '{}'::jsonb;

CREATE TABLE server_network_counters (
    server_id BIGINT PRIMARY KEY REFERENCES servers(id),
    checkpoint JSONB NOT NULL
);

CREATE TABLE server_network_daily (
    server_id BIGINT NOT NULL REFERENCES servers(id),
    day BIGINT NOT NULL,
    interface TEXT NOT NULL,
    uploaded NUMERIC(39,0) NOT NULL CHECK (uploaded >= 0),
    downloaded NUMERIC(39,0) NOT NULL CHECK (downloaded >= 0),
    first_sample_at BIGINT NOT NULL,
    last_sample_at BIGINT NOT NULL,
    incomplete BOOLEAN NOT NULL DEFAULT FALSE,
    PRIMARY KEY (server_id, day, interface)
);

CREATE FUNCTION sinan_traffic_cycle_start(at_time BIGINT, reset_day INTEGER)
RETURNS BIGINT LANGUAGE SQL IMMUTABLE STRICT AS $$
    WITH current_month AS (
        SELECT date_trunc('month', to_timestamp(at_time) AT TIME ZONE 'UTC')::date AS month
    ), candidate AS (
        SELECT month, month + (LEAST(reset_day,
            EXTRACT(DAY FROM month + INTERVAL '1 month - 1 day')::integer) - 1) AS boundary
        FROM current_month
    ), selected_month AS (
        SELECT CASE WHEN (to_timestamp(at_time) AT TIME ZONE 'UTC')::date >= boundary
            THEN month ELSE (month - INTERVAL '1 month')::date END AS month
        FROM candidate
    )
    SELECT EXTRACT(EPOCH FROM ((month + (LEAST(reset_day,
        EXTRACT(DAY FROM month + INTERVAL '1 month - 1 day')::integer) - 1))::timestamp
        AT TIME ZONE 'UTC'))::bigint FROM selected_month;
$$;
