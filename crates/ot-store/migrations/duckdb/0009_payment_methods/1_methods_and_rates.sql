-- Payment methods, as Frappe Books' Payment Method DocType, seeded with its Cash and
-- Bank methods; and exchange rates. Frappe Books fetches rates from a web service
-- for each invoice; OpenTally keeps the rates you enter, so invoices in another
-- currency work offline and the rate used stays on record. Portable SQL.
CREATE SEQUENCE IF NOT EXISTS payment_method_ids START 1;
CREATE TABLE IF NOT EXISTS payment_methods (
    id BIGINT PRIMARY KEY DEFAULT nextval('payment_method_ids'),
    name VARCHAR NOT NULL,
    name_key VARCHAR NOT NULL UNIQUE,
    method_type VARCHAR NOT NULL CHECK (method_type IN ('cash', 'bank', 'transfer')),
    -- The ledger payments by this method go to; chosen on the payment when empty.
    ledger_id BIGINT,
    requires_clearance_date BOOLEAN NOT NULL DEFAULT FALSE,
    revision INTEGER NOT NULL DEFAULT 1
);

INSERT INTO payment_methods (name, name_key, method_type) VALUES
    ('Cash', 'cash', 'cash'),
    ('Bank', 'bank', 'bank');

-- One unit of from_currency is `rate` units of to_currency on rate_date.
CREATE TABLE IF NOT EXISTS exchange_rates (
    from_currency VARCHAR NOT NULL,
    to_currency VARCHAR NOT NULL,
    rate_date DATE NOT NULL,
    rate DECIMAL(18, 9) NOT NULL CHECK (rate > 0),
    PRIMARY KEY (from_currency, to_currency, rate_date)
);
