-- The chart of accounts. Groups are group accounts and ledgers are ledger accounts,
-- as in Tally; these side tables add Frappe Books' root types, account types and GST
-- heads without altering the tables that postings reference. Portable SQL.
CREATE TABLE IF NOT EXISTS group_accounts (
    group_id BIGINT PRIMARY KEY REFERENCES account_groups(id),
    root_type VARCHAR CHECK (root_type IN ('Asset', 'Liability', 'Equity', 'Income', 'Expense')),
    account_type VARCHAR
);

CREATE TABLE IF NOT EXISTS ledger_accounts (
    ledger_id BIGINT PRIMARY KEY REFERENCES ledgers(id),
    account_type VARCHAR,
    gst_head VARCHAR CHECK (gst_head IN ('CGST', 'SGST', 'IGST', 'Exempt'))
);

-- Frappe Books' Accounting Settings: one row.
CREATE TABLE IF NOT EXISTS accounting_settings (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    country VARCHAR,
    currency VARCHAR NOT NULL DEFAULT 'INR',
    fiscal_year_start DATE,
    chart VARCHAR,
    gstin VARCHAR,
    round_off_ledger_id BIGINT REFERENCES ledgers(id),
    write_off_ledger_id BIGINT REFERENCES ledgers(id),
    discount_ledger_id BIGINT REFERENCES ledgers(id),
    receivable_ledger_id BIGINT REFERENCES ledgers(id),
    payable_ledger_id BIGINT REFERENCES ledgers(id),
    setup_complete BOOLEAN NOT NULL DEFAULT FALSE
);
INSERT INTO accounting_settings (id) VALUES (1) ON CONFLICT DO NOTHING;
