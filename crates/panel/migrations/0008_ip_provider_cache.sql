ALTER TABLE server_ip_quality ADD COLUMN provider TEXT NOT NULL DEFAULT 'check-place';
ALTER TABLE server_ip_quality ADD COLUMN last_attempt_at BIGINT;
ALTER TABLE server_ip_quality ADD COLUMN last_success_at BIGINT;
ALTER TABLE server_ip_quality ADD COLUMN fresh_until BIGINT;
ALTER TABLE server_ip_quality ADD COLUMN last_error JSONB NOT NULL DEFAULT '{}'::jsonb;
ALTER TABLE server_ip_quality DROP CONSTRAINT server_ip_quality_pkey;
ALTER TABLE server_ip_quality ADD PRIMARY KEY(server_id,ip,provider);
UPDATE server_ip_quality SET last_attempt_at=checked_at;

CREATE TABLE server_ip_quality_datasets (
    server_id BIGINT NOT NULL,
    ip TEXT NOT NULL,
    provider TEXT NOT NULL,
    database TEXT NOT NULL,
    checked_at BIGINT NOT NULL,
    last_attempt JSONB NOT NULL,
    last_attempt_at BIGINT,
    last_success_at BIGINT,
    fresh_until BIGINT,
    last_error JSONB,
    success_payload JSONB,
    PRIMARY KEY(server_id,ip,provider,database),
    FOREIGN KEY(server_id,ip,provider) REFERENCES server_ip_quality(server_id,ip,provider)
);

INSERT INTO server_ip_quality_datasets (
    server_id,ip,provider,database,checked_at,last_attempt,last_attempt_at,
    last_success_at,fresh_until,last_error,success_payload
)
SELECT cache.server_id,cache.ip,cache.provider,dataset->>'database',cache.checked_at,dataset,
    (dataset->>'attempted_at')::BIGINT,
    CASE WHEN dataset->>'status'='succeeded' AND jsonb_array_length(dataset->'fields')>0
        THEN COALESCE((dataset->>'attempted_at')::BIGINT,cache.checked_at) END,
    CASE WHEN dataset->>'status'='succeeded' AND jsonb_array_length(dataset->'fields')>0
        THEN (cache.payload->>'expires_at')::BIGINT END,
    CASE WHEN dataset->>'error' IS NOT NULL THEN jsonb_build_object(
        'kind',dataset->'error_kind','message',dataset->'error','http_status',dataset->'http_status',
        'attempted_at',dataset->'attempted_at','elapsed_ms',dataset->'elapsed_ms') END,
    CASE WHEN dataset->>'status'='succeeded' AND jsonb_array_length(dataset->'fields')>0
        THEN dataset END
FROM server_ip_quality AS cache,
    LATERAL jsonb_array_elements(cache.payload->'databases') AS dataset;

UPDATE server_ip_quality AS cache SET
    last_success_at=summary.last_success_at,fresh_until=summary.fresh_until,last_error=summary.last_error
FROM (
    SELECT server_id,ip,provider,MAX(last_success_at) AS last_success_at,
        CASE WHEN COUNT(success_payload)=COUNT(*) THEN MIN(fresh_until) END AS fresh_until,
        COALESCE(jsonb_object_agg(database,last_error) FILTER(WHERE last_error IS NOT NULL),'{}'::jsonb) AS last_error
    FROM server_ip_quality_datasets GROUP BY server_id,ip,provider
) AS summary
WHERE cache.server_id=summary.server_id AND cache.ip=summary.ip AND cache.provider=summary.provider;
