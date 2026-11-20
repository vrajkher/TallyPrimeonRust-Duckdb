-- Number series that name documents, as Frappe Books' Number Series DocType. The
-- prefix is the series' name. `current_number` is the last number taken, so a new
-- series has start_number - 1. One series per document type is the default.
-- Portable SQL.
CREATE TABLE IF NOT EXISTS number_series (
    prefix VARCHAR PRIMARY KEY,
    reference_type VARCHAR NOT NULL,
    start_number BIGINT NOT NULL,
    pad_zeros INTEGER NOT NULL,
    current_number BIGINT NOT NULL,
    is_default BOOLEAN NOT NULL DEFAULT FALSE
);

-- Frappe Books' standard series, from 1001 with four digits.
INSERT INTO number_series (prefix, reference_type, start_number, pad_zeros, current_number, is_default)
VALUES
    ('JV-', 'journal_entry', 1001, 4, 1000, TRUE),
    ('PAY-', 'payment', 1001, 4, 1000, TRUE),
    ('PINV-', 'purchase_invoice', 1001, 4, 1000, TRUE),
    ('PRLE-', 'pricing_rule', 1001, 4, 1000, TRUE),
    ('PREC-', 'purchase_receipt', 1001, 4, 1000, TRUE),
    ('SHPM-', 'shipment', 1001, 4, 1000, TRUE),
    ('SINV-', 'sales_invoice', 1001, 4, 1000, TRUE),
    ('SMOV-', 'stock_movement', 1001, 4, 1000, TRUE),
    ('SQUOT-', 'sales_quote', 1001, 4, 1000, TRUE);
