-- Payments confirmed through "renew" (ADR 0096). Additive: existing
-- subscriptions, reminders and imports are unchanged.
CREATE TABLE expense_payments (
    id BIGSERIAL PRIMARY KEY,
    subscription_id BIGINT NOT NULL REFERENCES expense_subscriptions(id) ON DELETE CASCADE,
    request_id UUID NOT NULL UNIQUE,
    period_start DATE NOT NULL,
    paid_on DATE NOT NULL,
    amount_units BIGINT NOT NULL CHECK(amount_units BETWEEN 0 AND 9000000000000000),
    currency TEXT NOT NULL CHECK(currency ~ '^[A-Z]{3}$'),
    administrator_id BIGINT NOT NULL,
    created_at BIGINT NOT NULL
);
CREATE INDEX expense_payments_subscription ON expense_payments(subscription_id,paid_on DESC);
