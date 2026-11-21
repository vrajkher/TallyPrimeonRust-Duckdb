-- The setup wizard records the whole fiscal year and the company's bank, as Frappe
-- Books' Accounting Settings do. Portable SQL.
ALTER TABLE accounting_settings ADD COLUMN fiscal_year_end DATE;
ALTER TABLE accounting_settings ADD COLUMN bank_ledger_id BIGINT;
