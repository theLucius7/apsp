CREATE INDEX server_network_daily_day_idx ON server_network_daily(day, server_id);
CREATE INDEX usage_records_period_idx ON usage_records(period_end);
