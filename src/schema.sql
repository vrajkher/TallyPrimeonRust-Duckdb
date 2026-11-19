CREATE TABLE IF NOT EXISTS book_settings (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    company_name VARCHAR NOT NULL,
    source_company VARCHAR,
    closed_through DATE
);
INSERT INTO book_settings VALUES (1, 'Local book', NULL, NULL) ON CONFLICT DO NOTHING;

CREATE SEQUENCE IF NOT EXISTS group_ids START 1;
CREATE TABLE IF NOT EXISTS account_groups (
    id BIGINT PRIMARY KEY DEFAULT nextval('group_ids'),
    name VARCHAR NOT NULL,
    name_key VARCHAR NOT NULL UNIQUE,
    parent_id BIGINT REFERENCES account_groups(id)
);
CREATE TABLE IF NOT EXISTS ledger_groups (
    ledger_id BIGINT PRIMARY KEY REFERENCES ledgers(id),
    group_id BIGINT NOT NULL REFERENCES account_groups(id)
);
CREATE TABLE IF NOT EXISTS ledger_openings (
    ledger_id BIGINT NOT NULL REFERENCES ledgers(id),
    opening_date DATE NOT NULL,
    signed_paise BIGINT NOT NULL,
    source VARCHAR NOT NULL,
    PRIMARY KEY (ledger_id, opening_date)
);

CREATE TABLE IF NOT EXISTS book_vouchers (
    id BIGINT PRIMARY KEY DEFAULT nextval('voucher_ids'),
    kind VARCHAR NOT NULL CHECK (kind IN ('payment', 'receipt', 'journal', 'contra', 'other')),
    local_number BIGINT,
    source_guid VARCHAR UNIQUE,
    CHECK (local_number IS NOT NULL OR source_guid IS NOT NULL),
    UNIQUE (kind, local_number)
);

CREATE TABLE IF NOT EXISTS voucher_revisions (
    voucher_id BIGINT NOT NULL REFERENCES book_vouchers(id),
    revision INTEGER NOT NULL CHECK (revision > 0),
    request_key VARCHAR UNIQUE,
    source_revision BIGINT,
    code VARCHAR NOT NULL,
    type_name VARCHAR NOT NULL,
    series VARCHAR NOT NULL,
    voucher_date DATE NOT NULL,
    status VARCHAR NOT NULL CHECK (status IN ('posted', 'cancelled', 'optional')),
    cash_ledger_id BIGINT REFERENCES ledgers(id),
    amount_paise HUGEINT NOT NULL CHECK (amount_paise >= 0),
    reference VARCHAR NOT NULL,
    narration VARCHAR NOT NULL,
    actor VARCHAR NOT NULL,
    action VARCHAR NOT NULL,
    reason VARCHAR NOT NULL,
    recorded_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (voucher_id, revision)
);

CREATE TABLE IF NOT EXISTS posting_lines (
    voucher_id BIGINT NOT NULL,
    revision INTEGER NOT NULL,
    line_no INTEGER NOT NULL CHECK (line_no > 0),
    ledger_id BIGINT NOT NULL REFERENCES ledgers(id),
    side VARCHAR NOT NULL CHECK (side IN ('debit', 'credit')),
    amount_paise BIGINT NOT NULL CHECK (amount_paise > 0),
    narration VARCHAR NOT NULL DEFAULT '',
    PRIMARY KEY (voucher_id, revision, line_no),
    FOREIGN KEY (voucher_id, revision) REFERENCES voucher_revisions(voucher_id, revision)
);

CREATE TABLE IF NOT EXISTS cost_allocations (
    voucher_id BIGINT NOT NULL,
    revision INTEGER NOT NULL,
    line_no INTEGER NOT NULL,
    ordinal INTEGER NOT NULL,
    category VARCHAR NOT NULL,
    centre VARCHAR NOT NULL,
    amount_paise BIGINT NOT NULL CHECK (amount_paise > 0),
    PRIMARY KEY (voucher_id, revision, line_no, ordinal),
    FOREIGN KEY (voucher_id, revision, line_no) REFERENCES posting_lines(voucher_id, revision, line_no)
);

CREATE TABLE IF NOT EXISTS import_batches (
    fingerprint VARCHAR PRIMARY KEY,
    source_company VARCHAR NOT NULL,
    format VARCHAR NOT NULL,
    payload VARCHAR NOT NULL,
    inserted INTEGER NOT NULL,
    revised INTEGER NOT NULL,
    duplicates INTEGER NOT NULL,
    imported_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS source_voucher_types (
    name_key VARCHAR PRIMARY KEY,
    name VARCHAR NOT NULL,
    parent VARCHAR NOT NULL
);

CREATE OR REPLACE VIEW current_vouchers AS
SELECT v.id, v.kind, v.local_number, v.source_guid, r.* EXCLUDE (voucher_id)
FROM book_vouchers v JOIN voucher_revisions r ON r.voucher_id = v.id
QUALIFY row_number() OVER (PARTITION BY v.id ORDER BY r.revision DESC) = 1;

CREATE OR REPLACE VIEW active_postings AS
SELECT v.id AS voucher_id, v.revision, v.code, v.voucher_date, v.kind,
       p.line_no, p.ledger_id, p.side, p.amount_paise, p.narration
FROM current_vouchers v
JOIN posting_lines p ON p.voucher_id = v.id AND p.revision = v.revision
WHERE v.status = 'posted';

CREATE OR REPLACE VIEW book_register AS
WITH summaries AS (
    SELECT p.voucher_id, p.revision,
           string_agg(DISTINCT l.name, ', ' ORDER BY l.name)
               FILTER (WHERE p.ledger_id IS DISTINCT FROM v.cash_ledger_id) AS ledger_names,
           first(l.name ORDER BY p.line_no) FILTER (WHERE p.side = 'debit') AS debit,
           first(l.name ORDER BY p.line_no) FILTER (WHERE p.side = 'credit') AS credit
    FROM posting_lines p
    JOIN current_vouchers v ON p.voucher_id = v.id AND p.revision = v.revision
    JOIN ledgers l ON l.id = p.ledger_id
    GROUP BY p.voucher_id, p.revision
)
SELECT v.*, coalesce(cash.name, '') AS cash_account, coalesce(s.ledger_names, '') AS other_account,
       CASE WHEN v.kind = 'journal' THEN '' ELSE coalesce(s.debit, '') END AS debit,
       CASE WHEN v.kind = 'journal' THEN '' ELSE coalesce(s.credit, '') END AS credit
FROM current_vouchers v
LEFT JOIN summaries s ON s.voucher_id = v.id AND s.revision = v.revision
LEFT JOIN ledgers cash ON cash.id = v.cash_ledger_id;

CREATE SEQUENCE IF NOT EXISTS bank_statement_ids START 1;
CREATE TABLE IF NOT EXISTS bank_statement_lines (
    id BIGINT PRIMARY KEY DEFAULT nextval('bank_statement_ids'),
    ledger_id BIGINT NOT NULL REFERENCES ledgers(id),
    transaction_id VARCHAR NOT NULL,
    statement_date DATE NOT NULL,
    signed_paise BIGINT NOT NULL CHECK (signed_paise <> 0),
    reference VARCHAR NOT NULL,
    description VARCHAR NOT NULL,
    UNIQUE (ledger_id, transaction_id)
);
CREATE SEQUENCE IF NOT EXISTS bank_match_ids START 1;
CREATE TABLE IF NOT EXISTS bank_matches (
    id BIGINT PRIMARY KEY DEFAULT nextval('bank_match_ids'),
    ledger_id BIGINT NOT NULL REFERENCES ledgers(id),
    actor VARCHAR NOT NULL,
    note VARCHAR NOT NULL,
    recorded_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE TABLE IF NOT EXISTS bank_match_lines (
    match_id BIGINT NOT NULL REFERENCES bank_matches(id),
    statement_id BIGINT NOT NULL REFERENCES bank_statement_lines(id),
    PRIMARY KEY (match_id, statement_id)
);
CREATE TABLE IF NOT EXISTS bank_match_postings (
    match_id BIGINT NOT NULL REFERENCES bank_matches(id),
    voucher_id BIGINT NOT NULL,
    revision INTEGER NOT NULL,
    line_no INTEGER NOT NULL,
    PRIMARY KEY (match_id, voucher_id, revision, line_no),
    FOREIGN KEY (voucher_id, revision, line_no) REFERENCES posting_lines(voucher_id, revision, line_no)
);
CREATE TABLE IF NOT EXISTS bank_match_undos (
    match_id BIGINT PRIMARY KEY REFERENCES bank_matches(id),
    actor VARCHAR NOT NULL,
    reason VARCHAR NOT NULL,
    recorded_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE OR REPLACE VIEW active_bank_matches AS
SELECT m.* FROM bank_matches m
WHERE NOT EXISTS (SELECT 1 FROM bank_match_undos u WHERE u.match_id = m.id);

CREATE TABLE IF NOT EXISTS period_closures (
    closed_through DATE PRIMARY KEY,
    actor VARCHAR NOT NULL,
    reason VARCHAR NOT NULL,
    recorded_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);
