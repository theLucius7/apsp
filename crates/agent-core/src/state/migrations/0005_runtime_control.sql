CREATE TABLE IF NOT EXISTS runtime_control (
    request_id TEXT PRIMARY KEY NOT NULL,
    digest TEXT NOT NULL,
    kind TEXT NOT NULL,
    module TEXT NOT NULL,
    request TEXT NOT NULL,
    result TEXT,
    acknowledged INTEGER NOT NULL DEFAULT 0 CHECK (acknowledged IN (0, 1))
);
CREATE INDEX IF NOT EXISTS runtime_control_pending ON runtime_control(acknowledged, result);
CREATE TABLE IF NOT EXISTS runtime_revision_floors (
    module TEXT PRIMARY KEY NOT NULL,
    revision TEXT NOT NULL
);
