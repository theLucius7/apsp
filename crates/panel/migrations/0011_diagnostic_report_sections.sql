ALTER TABLE diagnostic_jobs
    ADD COLUMN expected_sections TEXT[] NOT NULL DEFAULT '{}',
    ADD COLUMN report_completeness TEXT NOT NULL DEFAULT 'empty'
        CHECK (report_completeness IN ('empty', 'partial', 'complete', 'legacy'));

-- Preserve old report payloads and do not invent chapter-level evidence.
UPDATE diagnostic_jobs SET report_completeness='legacy' WHERE report IS NOT NULL;

CREATE TABLE diagnostic_report_sections (
    job_id UUID NOT NULL REFERENCES diagnostic_jobs(id) ON DELETE CASCADE,
    name TEXT NOT NULL CHECK (name ~ '^[a-z0-9_]{1,64}$'),
    text TEXT NOT NULL CHECK (octet_length(text) BETWEEN 1 AND 65536),
    complete BOOLEAN NOT NULL,
    revision BIGINT NOT NULL CHECK (revision > 0),
    collected_at BIGINT NOT NULL CHECK (collected_at > 0),
    received_at BIGINT NOT NULL,
    PRIMARY KEY (job_id, name)
);
