CREATE TABLE exchange_rates (
    singleton BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (singleton),
    base TEXT NOT NULL DEFAULT 'CNY' CHECK (base = 'CNY'),
    rates JSONB NOT NULL DEFAULT '{"CNY":1}',
    rate_dates JSONB NOT NULL DEFAULT '{}',
    source TEXT CHECK (source IN ('frankfurter','frankfurter-ecb')),
    rate_date TEXT,
    fetched_at BIGINT,
    attempted_at BIGINT,
    next_refresh_at BIGINT NOT NULL DEFAULT 0,
    lease_id UUID,
    lease_until BIGINT NOT NULL DEFAULT 0,
    last_error TEXT
);
INSERT INTO exchange_rates(singleton) VALUES(TRUE);
