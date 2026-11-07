ALTER TABLE usage_records DROP CONSTRAINT usage_records_server_id_epoch_seq_fkey;
ALTER TABLE usage_batches ALTER COLUMN seq TYPE NUMERIC(20, 0);
ALTER TABLE usage_records ALTER COLUMN seq TYPE NUMERIC(20, 0);
ALTER TABLE usage_records ALTER COLUMN uplink TYPE NUMERIC(20, 0);
ALTER TABLE usage_records ALTER COLUMN downlink TYPE NUMERIC(20, 0);
ALTER TABLE usage_batches ADD CHECK (seq <= 18446744073709551615);
ALTER TABLE usage_records ADD CHECK (seq <= 18446744073709551615);
ALTER TABLE usage_records ADD CHECK (uplink <= 18446744073709551615);
ALTER TABLE usage_records ADD CHECK (downlink <= 18446744073709551615);
ALTER TABLE usage_records ADD FOREIGN KEY (server_id, epoch, seq)
    REFERENCES usage_batches(server_id, epoch, seq);
