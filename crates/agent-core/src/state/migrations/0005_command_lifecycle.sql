CREATE TABLE IF NOT EXISTS command_lifecycle (
    id TEXT PRIMARY KEY REFERENCES command_journal(id) ON DELETE CASCADE,
    claim_id TEXT NOT NULL,
    process TEXT,
    started TEXT,
    started_acknowledged INTEGER NOT NULL DEFAULT 0,
    cancel_requested INTEGER NOT NULL DEFAULT 0,
    recovering INTEGER NOT NULL DEFAULT 0
);
