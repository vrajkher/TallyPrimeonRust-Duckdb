-- Tax templates, as Frappe Books' Tax and Tax Detail DocTypes: a named list of tax
-- rows, each posting a rate to a ledger. Items and item groups may name one. Links
-- are checked by the store rather than by foreign keys. Portable SQL.
CREATE SEQUENCE IF NOT EXISTS tax_ids START 1;
CREATE TABLE IF NOT EXISTS taxes (
    id BIGINT PRIMARY KEY DEFAULT nextval('tax_ids'),
    name VARCHAR NOT NULL,
    name_key VARCHAR NOT NULL UNIQUE,
    revision INTEGER NOT NULL DEFAULT 1
);

CREATE TABLE IF NOT EXISTS tax_details (
    tax_id BIGINT NOT NULL,
    -- The row's place in the template, from 1.
    idx INTEGER NOT NULL,
    ledger_id BIGINT NOT NULL,
    -- Where the tax is paid from, when it differs from where it is booked.
    payment_ledger_id BIGINT,
    -- Percent, e.g. 9 or 0.125.
    rate DECIMAL(9, 4) NOT NULL CHECK (rate >= 0 AND rate <= 100),
    PRIMARY KEY (tax_id, idx)
);

ALTER TABLE items ADD COLUMN tax_id BIGINT;
ALTER TABLE item_groups ADD COLUMN tax_id BIGINT;
