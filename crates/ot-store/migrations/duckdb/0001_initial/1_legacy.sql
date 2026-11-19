CREATE SEQUENCE IF NOT EXISTS ledger_ids START 1;
CREATE SEQUENCE IF NOT EXISTS voucher_ids START 1;

CREATE TABLE IF NOT EXISTS ledgers (
    id BIGINT PRIMARY KEY DEFAULT nextval('ledger_ids'),
    name VARCHAR NOT NULL CHECK (length(trim(name)) BETWEEN 1 AND 100),
    name_key VARCHAR NOT NULL UNIQUE,
    kind VARCHAR NOT NULL CHECK (kind IN ('cash', 'bank', 'general'))
);

CREATE TABLE IF NOT EXISTS vouchers (
    id BIGINT PRIMARY KEY DEFAULT nextval('voucher_ids'),
    request_id UUID NOT NULL UNIQUE,
    kind VARCHAR NOT NULL CHECK (kind IN ('payment', 'receipt')),
    number BIGINT NOT NULL CHECK (number > 0),
    voucher_date DATE NOT NULL,
    cash_ledger_id BIGINT NOT NULL REFERENCES ledgers(id),
    other_ledger_id BIGINT NOT NULL REFERENCES ledgers(id),
    amount_paise BIGINT NOT NULL CHECK (amount_paise > 0),
    reference VARCHAR NOT NULL CHECK (length(reference) <= 100),
    narration VARCHAR NOT NULL CHECK (length(narration) <= 500),
    CHECK (cash_ledger_id <> other_ledger_id),
    UNIQUE (kind, number)
);

CREATE TABLE IF NOT EXISTS journal_vouchers (
    id BIGINT PRIMARY KEY DEFAULT nextval('voucher_ids'),
    request_id UUID NOT NULL UNIQUE,
    number BIGINT NOT NULL UNIQUE CHECK (number > 0),
    voucher_date DATE NOT NULL,
    amount_paise HUGEINT NOT NULL CHECK (amount_paise > 0),
    reference VARCHAR NOT NULL CHECK (length(reference) <= 100),
    narration VARCHAR NOT NULL CHECK (length(narration) <= 500)
);

CREATE TABLE IF NOT EXISTS journal_postings (
    voucher_id BIGINT NOT NULL REFERENCES journal_vouchers(id),
    line_no INTEGER NOT NULL CHECK (line_no > 0),
    ledger_id BIGINT NOT NULL REFERENCES ledgers(id),
    side VARCHAR NOT NULL CHECK (side IN ('debit', 'credit')),
    amount_paise BIGINT NOT NULL CHECK (amount_paise > 0),
    PRIMARY KEY (voucher_id, line_no)
);

INSERT INTO ledgers (name, name_key, kind)
VALUES ('Cash', 'cash', 'cash'), ('Bank', 'bank', 'bank')
ON CONFLICT (name_key) DO NOTHING;

CREATE OR REPLACE VIEW voucher_register AS
SELECT
    v.id,
    v.kind,
    CASE v.kind WHEN 'payment' THEN 'PAY-' ELSE 'REC-' END ||
        CASE WHEN v.number < 1000000 THEN lpad(v.number::VARCHAR, 6, '0')
            ELSE v.number::VARCHAR END AS code,
    v.voucher_date,
    cash.name AS cash_account,
    other.name AS other_account,
    CASE WHEN v.kind = 'receipt' THEN cash.name ELSE other.name END AS debit,
    CASE WHEN v.kind = 'receipt' THEN other.name ELSE cash.name END AS credit,
    v.amount_paise,
    v.reference,
    v.narration
FROM vouchers AS v
JOIN ledgers AS cash ON cash.id = v.cash_ledger_id
JOIN ledgers AS other ON other.id = v.other_ledger_id

UNION ALL

SELECT
    v.id,
    'journal',
    'JRN-' || CASE WHEN v.number < 1000000 THEN lpad(v.number::VARCHAR, 6, '0')
        ELSE v.number::VARCHAR END,
    v.voucher_date,
    '',
    particulars.ledger_names,
    '',
    '',
    v.amount_paise,
    v.reference,
    v.narration
FROM journal_vouchers AS v
JOIN (
    SELECT p.voucher_id, string_agg(DISTINCT l.name, ', ' ORDER BY l.name) AS ledger_names
    FROM journal_postings AS p
    JOIN ledgers AS l ON l.id = p.ledger_id
    GROUP BY p.voucher_id
) AS particulars ON particulars.voucher_id = v.id;
