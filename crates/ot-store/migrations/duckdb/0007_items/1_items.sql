-- Items, item groups and units of measure, as Frappe Books' Item, Item Group, UOM and
-- UOM Conversion Item DocTypes. Links are checked by the store rather than by foreign
-- keys, so these tables can still be altered later. Portable SQL.
CREATE TABLE IF NOT EXISTS units (
    name VARCHAR PRIMARY KEY,
    name_key VARCHAR NOT NULL UNIQUE,
    -- Quantities in a whole unit must be whole numbers.
    is_whole BOOLEAN NOT NULL DEFAULT FALSE
);

-- Frappe Books' default units.
INSERT INTO units (name, name_key, is_whole) VALUES
    ('Unit', 'unit', TRUE),
    ('Kg', 'kg', FALSE),
    ('Gram', 'gram', FALSE),
    ('Meter', 'meter', FALSE),
    ('Hour', 'hour', FALSE),
    ('Day', 'day', FALSE);

CREATE SEQUENCE IF NOT EXISTS item_group_ids START 1;
CREATE TABLE IF NOT EXISTS item_groups (
    id BIGINT PRIMARY KEY DEFAULT nextval('item_group_ids'),
    name VARCHAR NOT NULL,
    name_key VARCHAR NOT NULL UNIQUE,
    hsn_code VARCHAR
);

CREATE SEQUENCE IF NOT EXISTS item_ids START 1;
CREATE TABLE IF NOT EXISTS items (
    id BIGINT PRIMARY KEY DEFAULT nextval('item_ids'),
    name VARCHAR NOT NULL,
    name_key VARCHAR NOT NULL UNIQUE,
    item_code VARCHAR,
    group_id BIGINT,
    item_usage VARCHAR NOT NULL DEFAULT 'both' CHECK (item_usage IN ('purchases', 'sales', 'both')),
    item_type VARCHAR NOT NULL DEFAULT 'product' CHECK (item_type IN ('product', 'service')),
    unit VARCHAR NOT NULL DEFAULT 'Unit',
    -- The rate in the company currency's smallest unit (paise for INR).
    rate_minor BIGINT NOT NULL DEFAULT 0,
    description VARCHAR,
    income_ledger_id BIGINT NOT NULL,
    expense_ledger_id BIGINT NOT NULL,
    hsn_code VARCHAR,
    barcode VARCHAR,
    track_item BOOLEAN NOT NULL DEFAULT FALSE,
    revision INTEGER NOT NULL DEFAULT 1
);

-- Other units an item is bought or sold in: one `unit` is `conversion_factor` of the
-- item's own unit.
CREATE TABLE IF NOT EXISTS item_units (
    item_id BIGINT NOT NULL,
    unit VARCHAR NOT NULL,
    conversion_factor DECIMAL(18, 6) NOT NULL CHECK (conversion_factor > 0),
    PRIMARY KEY (item_id, unit)
);
