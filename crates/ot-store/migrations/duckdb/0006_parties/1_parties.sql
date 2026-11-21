-- Parties (customers and suppliers) and their addresses, as Frappe Books' Party and
-- Address DocTypes. Ledger and address links are checked by the store rather than by
-- foreign keys, so these tables can still be altered later. Portable SQL.
CREATE SEQUENCE IF NOT EXISTS address_ids START 1;
CREATE TABLE IF NOT EXISTS addresses (
    id BIGINT PRIMARY KEY DEFAULT nextval('address_ids'),
    address_line1 VARCHAR NOT NULL,
    address_line2 VARCHAR,
    city VARCHAR NOT NULL,
    state VARCHAR,
    country VARCHAR NOT NULL,
    postal_code VARCHAR,
    email VARCHAR,
    phone VARCHAR,
    -- Derived on save: the one-line address and, in India, the place of supply.
    display VARCHAR NOT NULL,
    place_of_supply VARCHAR
);

CREATE SEQUENCE IF NOT EXISTS party_ids START 1;
CREATE TABLE IF NOT EXISTS parties (
    id BIGINT PRIMARY KEY DEFAULT nextval('party_ids'),
    name VARCHAR NOT NULL,
    -- Lower-cased name, so two parties cannot differ only in case.
    name_key VARCHAR NOT NULL UNIQUE,
    role VARCHAR NOT NULL CHECK (role IN ('customer', 'supplier', 'both')),
    email VARCHAR,
    phone VARCHAR,
    address_id BIGINT,
    default_ledger_id BIGINT,
    currency VARCHAR NOT NULL,
    tax_id VARCHAR,
    gst_type VARCHAR NOT NULL DEFAULT 'unregistered'
        CHECK (gst_type IN ('unregistered', 'registered_regular', 'consumer')),
    gstin VARCHAR,
    -- Bumped on every change; a save must name the revision it started from.
    revision INTEGER NOT NULL DEFAULT 1,
    created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);
