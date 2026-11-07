CREATE INDEX probe_result_target_time_idx ON probe_results(server_id, probe_id, sampled_at DESC, id DESC);
