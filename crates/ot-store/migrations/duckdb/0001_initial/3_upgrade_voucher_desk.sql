INSERT INTO book_vouchers (id, kind, local_number)
SELECT id, kind, number FROM vouchers
UNION ALL SELECT id, 'journal', number FROM journal_vouchers;

INSERT INTO voucher_revisions (
    voucher_id, revision, request_key, code, type_name, series, voucher_date,
    status, cash_ledger_id, amount_paise, reference, narration, actor, action, reason
)
SELECT r.id, 1, coalesce(v.request_id, j.request_id)::VARCHAR, r.code,
       initcap_kind.label, 'Default', r.voucher_date, 'posted',
       v.cash_ledger_id, r.amount_paise, r.reference, r.narration,
       'Voucher Desk', 'migrated', 'Imported from the existing Voucher Desk database'
FROM voucher_register r
LEFT JOIN vouchers v ON r.id = v.id
LEFT JOIN journal_vouchers j ON r.id = j.id
JOIN (VALUES ('payment', 'Payment'), ('receipt', 'Receipt'), ('journal', 'Journal'))
    AS initcap_kind(kind, label) ON r.kind = initcap_kind.kind;

INSERT INTO posting_lines (voucher_id, revision, line_no, ledger_id, side, amount_paise)
SELECT id, 1, 1, CASE WHEN kind = 'payment' THEN other_ledger_id ELSE cash_ledger_id END,
       'debit', amount_paise FROM vouchers
UNION ALL
SELECT id, 1, 2, CASE WHEN kind = 'payment' THEN cash_ledger_id ELSE other_ledger_id END,
       'credit', amount_paise FROM vouchers
UNION ALL
SELECT voucher_id, 1, line_no, ledger_id, side, amount_paise FROM journal_postings;
