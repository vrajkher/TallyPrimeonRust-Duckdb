use std::path::Path;

use chrono::{Datelike, NaiveDate};
use duckdb::{Connection, OptionalExt, Row, Transaction, params};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

const PAGE_SIZE: u32 = 25;
const MAX_JOURNAL_LINES: usize = 50;
const REGISTER_FILTER: &str = "
    WHERE ($1::VARCHAR IS NULL OR kind = $1)
      AND ($2::DATE IS NULL OR voucher_date >= $2)
      AND ($3::DATE IS NULL OR voucher_date <= $3)
      AND ($4::VARCHAR IS NULL
        OR contains(lower(cash_account), lower($4))
        OR contains(lower(other_account), lower($4))
        OR contains(lower(reference), lower($4))
        OR contains(lower(narration), lower($4))
        OR contains(lower(code), lower($4)))";

const VOUCHER_COLUMNS: &str = "
    id, kind, code, voucher_date::VARCHAR, cash_account, other_account,
    debit, credit, amount_paise::VARCHAR, reference, narration";

#[derive(Debug)]
pub enum StoreError {
    Invalid(String),
    Conflict(String),
    Missing(String),
    Database(duckdb::Error),
    Unavailable,
}

impl From<duckdb::Error> for StoreError {
    fn from(error: duckdb::Error) -> Self {
        Self::Database(error)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LedgerKind {
    Cash,
    Bank,
    General,
}

impl LedgerKind {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Cash => "cash",
            Self::Bank => "bank",
            Self::General => "general",
        }
    }
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VoucherKind {
    Payment,
    Receipt,
    Journal,
}

impl VoucherKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Payment => "payment",
            Self::Receipt => "receipt",
            Self::Journal => "journal",
        }
    }
}

#[derive(Deserialize)]
pub struct NewLedger {
    name: String,
    kind: LedgerKind,
}

#[derive(Serialize)]
pub struct Ledger {
    id: i64,
    name: String,
    kind: String,
}

#[derive(Deserialize)]
pub struct NewVoucher {
    request_id: String,
    date: String,
    #[serde(default)]
    reference: String,
    #[serde(default)]
    narration: String,
    #[serde(flatten)]
    entry: VoucherEntry,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
enum VoucherEntry {
    Payment(CashVoucher),
    Receipt(CashVoucher),
    Journal { postings: Vec<NewPosting> },
}

impl VoucherEntry {
    fn kind(&self) -> VoucherKind {
        match self {
            Self::Payment(_) => VoucherKind::Payment,
            Self::Receipt(_) => VoucherKind::Receipt,
            Self::Journal { .. } => VoucherKind::Journal,
        }
    }
}

#[derive(Deserialize)]
struct CashVoucher {
    cash_ledger_id: i64,
    other_ledger_id: i64,
    amount: String,
}

#[derive(Deserialize)]
struct NewPosting {
    ledger_id: i64,
    side: PostingSide,
    amount: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum PostingSide {
    Debit,
    Credit,
}

struct VoucherDetails<'a> {
    request_id: String,
    date: &'a str,
    reference: &'a str,
    narration: &'a str,
}

#[derive(Debug, Serialize)]
pub struct Voucher {
    id: i64,
    kind: String,
    code: String,
    date: String,
    cash_account: String,
    other_account: String,
    debit: String,
    credit: String,
    amount_paise: String,
    reference: String,
    narration: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    postings: Vec<VoucherPosting>,
}

#[derive(Debug, Serialize)]
struct VoucherPosting {
    ledger_id: i64,
    ledger: String,
    side: String,
    amount_paise: String,
}

#[derive(Default, Deserialize)]
pub struct RegisterFilter {
    kind: Option<VoucherKind>,
    from: Option<String>,
    to: Option<String>,
    q: Option<String>,
    page: Option<u32>,
}

#[derive(Serialize)]
pub struct Register {
    rows: Vec<Voucher>,
    page: u32,
    page_size: u32,
    count: i64,
    receipt_paise: String,
    payment_paise: String,
}

pub struct Store {
    connection: Connection,
}

impl Store {
    pub fn open(path: &Path) -> duckdb::Result<Self> {
        Self::initialize(Connection::open(path)?)
    }

    fn initialize(mut connection: Connection) -> duckdb::Result<Self> {
        let tx = connection.transaction()?;
        tx.execute_batch(include_str!("schema.sql"))?;
        tx.commit()?;
        Ok(Self { connection })
    }

    pub fn ledgers(&self) -> Result<Vec<Ledger>, StoreError> {
        let mut stmt = self
            .connection
            .prepare("SELECT id, name, kind FROM ledgers ORDER BY name_key")?;
        let ledgers = stmt.query_map([], |row| {
            Ok(Ledger {
                id: row.get(0)?,
                name: row.get(1)?,
                kind: row.get(2)?,
            })
        })?;
        Ok(ledgers.collect::<duckdb::Result<_>>()?)
    }

    pub fn add_ledger(&self, entry: NewLedger) -> Result<Ledger, StoreError> {
        let name = validated_text(&entry.name, "Ledger name", 100)?;
        if name.is_empty() {
            return Err(StoreError::Invalid("Enter a ledger name.".into()));
        }
        let name_key = name.to_lowercase();
        let kind = entry.kind.as_str();
        let id = self
            .connection
            .query_row(
                "INSERT INTO ledgers (name, name_key, kind) VALUES (?, ?, ?)
                 ON CONFLICT (name_key) DO NOTHING RETURNING id",
                params![name, name_key, kind],
                |row| row.get(0),
            )
            .optional()?
            .ok_or_else(|| {
                StoreError::Conflict("A ledger with that name already exists.".into())
            })?;

        Ok(Ledger {
            id,
            name: name.into(),
            kind: kind.into(),
        })
    }

    pub fn voucher(&self, id: i64) -> Result<Voucher, StoreError> {
        read_voucher(&self.connection, id)
    }

    pub fn add_voucher(&mut self, entry: NewVoucher) -> Result<(Voucher, bool), StoreError> {
        let details = VoucherDetails {
            request_id: Uuid::parse_str(&entry.request_id)
                .map_err(|_| StoreError::Invalid("Invalid voucher request ID.".into()))?
                .to_string(),
            date: validated_date(&entry.date)?,
            reference: validated_text(&entry.reference, "Reference", 100)?,
            narration: validated_text(&entry.narration, "Narration", 500)?,
        };
        let kind = entry.entry.kind().as_str();
        let tx = self.connection.transaction()?;
        let existing: Option<(i64, bool)> = tx
            .query_row(
                "SELECT id, kind = $2 AND voucher_date = $3::DATE
                        AND reference = $4 AND narration = $5
                 FROM (
                    SELECT id, kind, voucher_date, reference, narration
                    FROM vouchers WHERE request_id = $1::UUID
                    UNION ALL
                    SELECT id, 'journal', voucher_date, reference, narration
                    FROM journal_vouchers WHERE request_id = $1::UUID
                 ) AS existing",
                params![
                    details.request_id,
                    kind,
                    details.date,
                    details.reference,
                    details.narration
                ],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        if matches!(existing, Some((_, false))) {
            return Err(request_conflict());
        }
        let existing_id = existing.map(|(id, _)| id);

        let id = match entry.entry {
            VoucherEntry::Payment(cash) | VoucherEntry::Receipt(cash) => {
                save_cash_voucher(&tx, &details, kind, cash, existing_id)?
            }
            VoucherEntry::Journal { postings } => {
                save_journal(&tx, &details, postings, existing_id)?
            }
        };
        let voucher = read_voucher(&tx, id)?;
        tx.commit()?;
        Ok((voucher, existing.is_none()))
    }

    pub fn register(&self, filter: RegisterFilter) -> Result<Register, StoreError> {
        let kind = filter.kind.map(VoucherKind::as_str);
        let from = filter.from.as_deref().map(validated_date).transpose()?;
        let to = filter.to.as_deref().map(validated_date).transpose()?;
        if matches!((from, to), (Some(from), Some(to)) if from > to) {
            return Err(StoreError::Invalid(
                "The start date must be on or before the end date.".into(),
            ));
        }
        let q = filter
            .q
            .as_deref()
            .map(|text| validated_text(text, "Search", 100))
            .transpose()?
            .filter(|text| !text.is_empty());
        let page = filter.page.unwrap_or(1).max(1);
        let offset = i64::from(page - 1) * i64::from(PAGE_SIZE);

        let (count, receipt_paise, payment_paise) = self.connection.query_row(
            &format!(
                "SELECT COUNT(*),
                    COALESCE(SUM(amount_paise) FILTER (WHERE kind = 'receipt'), 0)::VARCHAR,
                    COALESCE(SUM(amount_paise) FILTER (WHERE kind = 'payment'), 0)::VARCHAR
                 FROM voucher_register {REGISTER_FILTER}"
            ),
            params![kind, from, to, q],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;

        let mut stmt = self.connection.prepare(&format!(
            "SELECT {VOUCHER_COLUMNS} FROM voucher_register {REGISTER_FILTER}
             ORDER BY voucher_date DESC, id DESC LIMIT $5 OFFSET $6"
        ))?;
        let rows = stmt
            .query_map(params![kind, from, to, q, PAGE_SIZE, offset], voucher_row)?
            .collect::<duckdb::Result<_>>()?;

        Ok(Register {
            rows,
            page,
            page_size: PAGE_SIZE,
            count,
            receipt_paise,
            payment_paise,
        })
    }
}

fn save_cash_voucher(
    tx: &Transaction<'_>,
    details: &VoucherDetails<'_>,
    kind: &str,
    entry: CashVoucher,
    existing_id: Option<i64>,
) -> Result<i64, StoreError> {
    let amount_paise = parse_amount(&entry.amount)?;
    if let Some(id) = existing_id {
        let matches: bool = tx.query_row(
            "SELECT cash_ledger_id = ? AND other_ledger_id = ? AND amount_paise = ?
             FROM vouchers WHERE id = ?",
            params![
                entry.cash_ledger_id,
                entry.other_ledger_id,
                amount_paise,
                id
            ],
            |row| row.get(0),
        )?;
        return if matches {
            Ok(id)
        } else {
            Err(request_conflict())
        };
    }

    let cash_kind = ledger_kind(tx, entry.cash_ledger_id)?;
    if !matches!(cash_kind.as_deref(), Some("cash" | "bank")) {
        return Err(StoreError::Invalid("Choose a cash or bank ledger.".into()));
    }
    let other_kind = ledger_kind(tx, entry.other_ledger_id)?;
    if other_kind.as_deref() != Some("general") {
        return Err(StoreError::Invalid(
            "Choose a party, expense, income, or other general ledger.".into(),
        ));
    }

    Ok(tx.query_row(
        "INSERT INTO vouchers (
            request_id, kind, number, voucher_date, cash_ledger_id, other_ledger_id,
            amount_paise, reference, narration
         ) SELECT ?::UUID, ?, COALESCE(MAX(number), 0) + 1, ?::DATE, ?, ?, ?, ?, ?
           FROM vouchers WHERE kind = ?
         RETURNING id",
        params![
            details.request_id,
            kind,
            details.date,
            entry.cash_ledger_id,
            entry.other_ledger_id,
            amount_paise,
            details.reference,
            details.narration,
            kind
        ],
        |row| row.get(0),
    )?)
}

fn save_journal(
    tx: &Transaction<'_>,
    details: &VoucherDetails<'_>,
    postings: Vec<NewPosting>,
    existing_id: Option<i64>,
) -> Result<i64, StoreError> {
    if !(2..=MAX_JOURNAL_LINES).contains(&postings.len()) {
        return Err(StoreError::Invalid(format!(
            "Enter 2 to {MAX_JOURNAL_LINES} journal lines."
        )));
    }

    let mut debit_total = 0i128;
    let mut credit_total = 0i128;
    let mut lines = Vec::with_capacity(postings.len());
    for (number, posting) in (1..).zip(postings) {
        let amount = parse_amount(&posting.amount).map_err(|_| {
            StoreError::Invalid(format!(
                "Enter a positive amount with up to two decimal places on line {number}."
            ))
        })?;
        let side = match posting.side {
            PostingSide::Debit => {
                debit_total += i128::from(amount);
                "debit"
            }
            PostingSide::Credit => {
                credit_total += i128::from(amount);
                "credit"
            }
        };
        lines.push((number, posting.ledger_id, side, amount));
    }
    if debit_total != credit_total {
        return Err(StoreError::Invalid(
            "Journal debits and credits must match.".into(),
        ));
    }

    if let Some(id) = existing_id {
        let saved = read_journal_postings(tx, id)?;
        let matches = saved.len() == lines.len()
            && saved
                .iter()
                .zip(&lines)
                .all(|(saved, &(_, ledger, side, amount))| {
                    saved.ledger_id == ledger
                        && saved.side == side
                        && saved.amount_paise == amount.to_string()
                });
        return if matches {
            Ok(id)
        } else {
            Err(request_conflict())
        };
    }

    for &(number, ledger, ..) in &lines {
        if ledger_kind(tx, ledger)?.is_none() {
            return Err(StoreError::Invalid(format!(
                "Choose an existing ledger on line {number}."
            )));
        }
    }

    let id = tx.query_row(
        "INSERT INTO journal_vouchers (request_id, number, voucher_date, amount_paise, reference, narration)
         SELECT ?::UUID, COALESCE(MAX(number), 0) + 1, ?::DATE, ?::HUGEINT, ?, ?
         FROM journal_vouchers RETURNING id",
        params![details.request_id, details.date, debit_total.to_string(), details.reference, details.narration],
        |row| row.get(0),
    )?;
    let mut insert = tx.prepare(
        "INSERT INTO journal_postings (voucher_id, line_no, ledger_id, side, amount_paise)
         VALUES (?, ?, ?, ?, ?)",
    )?;
    for (number, ledger, side, amount) in lines {
        insert.execute(params![id, number, ledger, side, amount])?;
    }
    Ok(id)
}

fn request_conflict() -> StoreError {
    StoreError::Conflict("This request ID is already associated with a different voucher.".into())
}

fn read_voucher(connection: &Connection, id: i64) -> Result<Voucher, StoreError> {
    let mut voucher = connection
        .query_row(
            &format!("SELECT {VOUCHER_COLUMNS} FROM voucher_register WHERE id = ?"),
            [id],
            voucher_row,
        )
        .optional()?
        .ok_or_else(|| StoreError::Missing("Voucher not found.".into()))?;
    if voucher.kind == "journal" {
        voucher.postings = read_journal_postings(connection, id)?;
    }
    Ok(voucher)
}

fn read_journal_postings(connection: &Connection, id: i64) -> duckdb::Result<Vec<VoucherPosting>> {
    let mut stmt = connection.prepare(
        "SELECT p.ledger_id, l.name, p.side, p.amount_paise::VARCHAR
         FROM journal_postings AS p JOIN ledgers AS l ON l.id = p.ledger_id
         WHERE p.voucher_id = ? ORDER BY p.line_no",
    )?;
    stmt.query_map([id], |row| {
        Ok(VoucherPosting {
            ledger_id: row.get(0)?,
            ledger: row.get(1)?,
            side: row.get(2)?,
            amount_paise: row.get(3)?,
        })
    })?
    .collect()
}

fn voucher_row(row: &Row<'_>) -> duckdb::Result<Voucher> {
    Ok(Voucher {
        id: row.get(0)?,
        kind: row.get(1)?,
        code: row.get(2)?,
        date: row.get(3)?,
        cash_account: row.get(4)?,
        other_account: row.get(5)?,
        debit: row.get(6)?,
        credit: row.get(7)?,
        amount_paise: row.get(8)?,
        reference: row.get(9)?,
        narration: row.get(10)?,
        postings: Vec::new(),
    })
}

fn ledger_kind(connection: &Connection, id: i64) -> duckdb::Result<Option<String>> {
    connection
        .query_row("SELECT kind FROM ledgers WHERE id = ?", [id], |row| {
            row.get(0)
        })
        .optional()
}

fn validated_text<'a>(text: &'a str, label: &str, max: usize) -> Result<&'a str, StoreError> {
    let text = text.trim();
    if text.chars().count() > max || text.contains('\0') {
        return Err(StoreError::Invalid(format!(
            "{label} must contain at most {max} characters and no null characters."
        )));
    }
    Ok(text)
}

fn validated_date(date: &str) -> Result<&str, StoreError> {
    if let Ok(parsed) = NaiveDate::parse_from_str(date, "%Y-%m-%d")
        && (1..=9999).contains(&parsed.year())
        && parsed.format("%Y-%m-%d").to_string() == date
    {
        return Ok(date);
    }
    Err(StoreError::Invalid(
        "Enter a valid date in YYYY-MM-DD format.".into(),
    ))
}

fn parse_amount(amount: &str) -> Result<i64, StoreError> {
    let invalid =
        || StoreError::Invalid("Enter a positive amount with up to two decimal places.".into());
    let (rupees, fraction) = amount
        .trim()
        .split_once('.')
        .map_or((amount.trim(), None), |(rupees, fraction)| {
            (rupees, Some(fraction))
        });
    if rupees.is_empty() || !rupees.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(invalid());
    }
    let paise = match fraction.map(str::as_bytes) {
        None => 0,
        Some([first]) if first.is_ascii_digit() => i64::from(first - b'0') * 10,
        Some([first, second]) if first.is_ascii_digit() && second.is_ascii_digit() => {
            i64::from(first - b'0') * 10 + i64::from(second - b'0')
        }
        _ => return Err(invalid()),
    };
    rupees
        .parse::<i64>()
        .ok()
        .and_then(|value| value.checked_mul(100))
        .and_then(|value| value.checked_add(paise))
        .filter(|value| *value > 0)
        .ok_or_else(invalid)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (Store, i64, i64) {
        let store = Store::initialize(Connection::open_in_memory().unwrap()).unwrap();
        let bank = store
            .add_ledger(NewLedger {
                name: "Bank of India".into(),
                kind: LedgerKind::Bank,
            })
            .unwrap();
        let conveyance = store
            .add_ledger(NewLedger {
                name: "Conveyance".into(),
                kind: LedgerKind::General,
            })
            .unwrap();
        (store, bank.id, conveyance.id)
    }

    fn payment(bank: i64, conveyance: i64, request_suffix: u8, amount: &str) -> NewVoucher {
        NewVoucher {
            request_id: format!("7c1d9589-3efd-4872-ac82-{request_suffix:012}"),
            date: "2026-11-18".into(),
            reference: "Conveyance".into(),
            narration: "".into(),
            entry: VoucherEntry::Payment(CashVoucher {
                cash_ledger_id: bank,
                other_ledger_id: conveyance,
                amount: amount.into(),
            }),
        }
    }

    #[test]
    fn conveyance_payments_and_receipt_post_to_expected_ledgers() {
        let (mut store, bank, conveyance) = fixture();
        for (number, amount, paise) in [(1, "12000", "1200000"), (2, "5000", "500000")] {
            let (voucher, created) = store
                .add_voucher(payment(bank, conveyance, number, amount))
                .unwrap();
            assert!(created);
            assert_eq!(voucher.code, format!("PAY-{number:06}"));
            assert_eq!(voucher.debit, "Conveyance");
            assert_eq!(voucher.credit, "Bank of India");
            assert_eq!(voucher.amount_paise, paise);
        }

        let mut receipt = payment(bank, conveyance, 3, "125.05");
        let VoucherEntry::Payment(cash) = receipt.entry else {
            unreachable!()
        };
        receipt.entry = VoucherEntry::Receipt(cash);
        receipt.date = "2026-11-20".into();
        let (voucher, _) = store.add_voucher(receipt).unwrap();
        assert_eq!(voucher.code, "REC-000001");
        assert_eq!(voucher.debit, "Bank of India");
        assert_eq!(voucher.credit, "Conveyance");
        assert_eq!(voucher.amount_paise, "12505");

        let register = store
            .register(RegisterFilter {
                from: Some("2026-11-18".into()),
                to: Some("2026-11-18".into()),
                q: Some("PAY-00000".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(register.count, 2);
        assert_eq!(register.payment_paise, "1700000");
        assert_eq!(register.receipt_paise, "0");
        assert_eq!(register.rows[0].code, "PAY-000002");

        let receipts = store
            .register(RegisterFilter {
                kind: Some(VoucherKind::Receipt),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(receipts.count, 1);
        assert_eq!(receipts.receipt_paise, "12505");
    }

    #[test]
    fn an_invalid_post_and_a_retried_request_do_not_create_extra_vouchers() {
        let (mut store, bank, conveyance) = fixture();
        assert!(matches!(
            store.add_voucher(payment(conveyance, bank, 1, "12.50")),
            Err(StoreError::Invalid(_))
        ));

        let (first, created) = store
            .add_voucher(payment(bank, conveyance, 1, "12.5"))
            .unwrap();
        assert!(created);
        let (retried, created) = store
            .add_voucher(payment(bank, conveyance, 1, "12.50"))
            .unwrap();
        assert!(!created);
        assert_eq!(first.id, retried.id);
        assert_eq!(first.code, "PAY-000001");

        assert!(matches!(
            store.add_voucher(payment(bank, conveyance, 1, "12.51")),
            Err(StoreError::Conflict(_))
        ));
        let register = store.register(RegisterFilter::default()).unwrap();
        assert_eq!(register.count, 1);
        assert_eq!(register.payment_paise, "1250");
        let (second, _) = store
            .add_voucher(payment(bank, conveyance, 2, "0.01"))
            .unwrap();
        assert_eq!(second.code, "PAY-000002");
    }

    #[test]
    fn journal_requires_balanced_lines_and_keeps_cash_totals_separate() {
        let (mut store, bank, conveyance) = fixture();
        let cash = store
            .ledgers()
            .unwrap()
            .into_iter()
            .find(|ledger| ledger.kind == "cash")
            .unwrap();
        let journal = |suffix, credit: &str, credit_ledger| {
            let mut entry = payment(bank, conveyance, suffix, "1");
            entry.entry = VoucherEntry::Journal {
                postings: vec![
                    NewPosting {
                        ledger_id: conveyance,
                        side: PostingSide::Debit,
                        amount: "10.25".into(),
                    },
                    NewPosting {
                        ledger_id: cash.id,
                        side: PostingSide::Debit,
                        amount: "0.75".into(),
                    },
                    NewPosting {
                        ledger_id: credit_ledger,
                        side: PostingSide::Credit,
                        amount: credit.into(),
                    },
                ],
            };
            entry
        };

        for invalid in [
            journal(1, "10.99", bank),
            journal(1, "11", -1),
            journal(1, "0", bank),
        ] {
            assert!(matches!(
                store.add_voucher(invalid),
                Err(StoreError::Invalid(_))
            ));
        }
        assert_eq!(store.register(RegisterFilter::default()).unwrap().count, 0);

        let (voucher, created) = store.add_voucher(journal(1, "11", bank)).unwrap();
        assert!(created);
        assert_eq!(voucher.code, "JRN-000001");
        assert_eq!(voucher.amount_paise, "1100");
        let postings: Vec<_> = voucher
            .postings
            .iter()
            .map(|p| (p.ledger.as_str(), p.side.as_str(), p.amount_paise.as_str()))
            .collect();
        assert_eq!(
            postings,
            [
                ("Conveyance", "debit", "1025"),
                ("Cash", "debit", "75"),
                ("Bank of India", "credit", "1100")
            ]
        );
        let (retried, created) = store.add_voucher(journal(1, "11.00", bank)).unwrap();
        assert!(!created);
        assert_eq!(retried.id, voucher.id);
        assert!(matches!(
            store.add_voucher(journal(1, "11", cash.id)),
            Err(StoreError::Conflict(_))
        ));
        assert!(matches!(
            store.add_voucher(payment(bank, conveyance, 1, "11")),
            Err(StoreError::Conflict(_))
        ));
        let (payment, _) = store
            .add_voucher(payment(bank, conveyance, 2, "2"))
            .unwrap();
        assert_eq!(payment.code, "PAY-000001");
        assert_ne!(voucher.id, payment.id);

        let register = store.register(RegisterFilter::default()).unwrap();
        assert_eq!(register.count, 2);
        assert_eq!(register.payment_paise, "200");
        assert_eq!(register.receipt_paise, "0");
        let journals = store
            .register(RegisterFilter {
                kind: Some(VoucherKind::Journal),
                q: Some("India".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(journals.count, 1);
        assert_eq!(journals.rows[0].id, voucher.id);
        assert_eq!(journals.payment_paise, "0");
    }

    #[test]
    fn money_requires_positive_whole_paise_and_detects_overflow() {
        assert_eq!(parse_amount("0.01").unwrap(), 1);
        assert_eq!(parse_amount("12.5").unwrap(), 1_250);
        assert_eq!(parse_amount("92233720368547758.07").unwrap(), i64::MAX);
        for invalid in [
            "0",
            "0.00",
            "1.001",
            "2.",
            "-1",
            "1e2",
            "1,000",
            "92233720368547758.08",
        ] {
            assert!(parse_amount(invalid).is_err(), "{invalid}");
        }
    }
}
