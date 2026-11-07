-- The last time the panel heard from a device. A clean disconnect backdates
-- last_seen past the online window so the device shows offline at once; offline
-- alerts measure their threshold from this real contact time instead (ADR 0084).
-- Rows without a value fall back to last_seen.
ALTER TABLE servers ADD COLUMN last_contact_at BIGINT;
