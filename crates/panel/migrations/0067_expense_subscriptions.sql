CREATE TABLE expense_settings (
    singleton BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK(singleton),
    base_currency TEXT NOT NULL DEFAULT 'USD' CHECK(base_currency ~ '^[A-Z]{3}$'),
    budget_units BIGINT CHECK(budget_units BETWEEN 0 AND 9000000000000000),
    timezone TEXT NOT NULL DEFAULT 'UTC',
    reminders_enabled BOOLEAN NOT NULL DEFAULT FALSE,
    version BIGINT NOT NULL DEFAULT 1
);
INSERT INTO expense_settings(singleton) VALUES(TRUE);
CREATE TABLE expense_labels (
    id BIGSERIAL PRIMARY KEY,
    kind TEXT NOT NULL CHECK(kind IN('category','payment_method','payer')),
    name TEXT NOT NULL,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    UNIQUE(kind,name)
);
CREATE TABLE expense_subscriptions (
    id BIGSERIAL PRIMARY KEY,
    name TEXT NOT NULL,
    amount_units BIGINT NOT NULL CHECK(amount_units BETWEEN 0 AND 9000000000000000),
    currency TEXT NOT NULL CHECK(currency ~ '^[A-Z]{3}$'),
    start_date DATE,
    next_payment DATE NOT NULL,
    anchor_payment DATE NOT NULL,
    cycle TEXT NOT NULL CHECK(cycle IN('day','week','month','year')),
    frequency INTEGER NOT NULL CHECK(frequency BETWEEN 1 AND 1000),
    category_id BIGINT REFERENCES expense_labels(id),
    payment_method_id BIGINT REFERENCES expense_labels(id),
    payer_id BIGINT REFERENCES expense_labels(id),
    notes TEXT NOT NULL DEFAULT '',
    url TEXT NOT NULL DEFAULT '',
    inactive BOOLEAN NOT NULL DEFAULT FALSE,
    auto_renew BOOLEAN NOT NULL DEFAULT TRUE,
    notify BOOLEAN NOT NULL DEFAULT FALSE,
    notify_days_before INTEGER NOT NULL DEFAULT 3 CHECK(notify_days_before BETWEEN 0 AND 365),
    cancellation_date DATE,
    replacement_id BIGINT REFERENCES expense_subscriptions(id),
    version BIGINT NOT NULL DEFAULT 1,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
    CHECK(replacement_id IS NULL OR replacement_id <> id),
    CHECK(start_date IS NULL OR start_date <= next_payment),
    CHECK(cancellation_date IS NULL OR start_date IS NULL OR cancellation_date >= start_date)
);
CREATE TABLE expense_events (
    id UUID PRIMARY KEY,
    subscription_id BIGINT REFERENCES expense_subscriptions(id) ON DELETE SET NULL,
    event_key TEXT NOT NULL UNIQUE,
    kind TEXT NOT NULL,
    detail JSONB NOT NULL,
    created_at BIGINT NOT NULL
);
CREATE TABLE expense_deliveries (
    id BIGSERIAL PRIMARY KEY,
    event_id UUID NOT NULL REFERENCES expense_events(id),
    channel TEXT NOT NULL CHECK(channel IN('telegram','webhook')),
    payload TEXT NOT NULL,
    state TEXT NOT NULL DEFAULT 'pending' CHECK(state IN('pending','sending','sent','failed','cancelled')),
    attempts INTEGER NOT NULL DEFAULT 0,
    next_attempt_at BIGINT NOT NULL,
    lease_until BIGINT,
    last_error TEXT,
    UNIQUE(event_id,channel)
);
CREATE TABLE expense_imports (
    id UUID PRIMARY KEY,
    source_id TEXT NOT NULL UNIQUE,
    digest TEXT NOT NULL,
    receipt JSONB NOT NULL,
    created_at BIGINT NOT NULL
);
