use std::{collections::HashSet, path::Path};

use chrono::{Datelike, NaiveDate};
use duckdb::{Connection, OptionalExt, Row, Transaction, params};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

const PAGE_SIZE: u32 = 25;
const MAX_POSTING_LINES: usize = 50;
const REGISTER_FILTER: &str = "
    WHERE ($1::VARCHAR IS NULL OR kind = $1)
      AND ($2::DATE IS NULL OR voucher_date >= $2)
      AND ($3::DATE IS NULL OR voucher_date <= $3)
      AND ($4::VARCHAR IS NULL
        OR contains(lower(cash_account), lower($4))
        OR contains(lower(other_account), lower($4))
        OR contains(lower(reference), lower($4))
        OR contains(lower(narration), lower($4))
        OR contains(lower(code), lower($4))
        OR contains(lower(type_name), lower($4)))
      AND ($5::VARCHAR IS NULL OR status = $5)";
const VOUCHER_COLUMNS: &str = "
    id, kind, code, voucher_date::VARCHAR, cash_account, other_account, debit, credit,
    amount_paise::VARCHAR, reference, narration, type_name, series, status, revision, source_guid";

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

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LedgerKind {
    Cash,
    Bank,
    General,
}

impl LedgerKind {
    pub(crate) fn as_str(self) -> &'static str {
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
    Contra,
    Other,
}

impl VoucherKind {
    fn labels(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Self::Payment => ("payment", "Payment", "PAY"),
            Self::Receipt => ("receipt", "Receipt", "REC"),
            Self::Journal => ("journal", "Journal", "JRN"),
            Self::Contra => ("contra", "Contra", "CON"),
            Self::Other => ("other", "Other", "OTH"),
        }
    }

    fn as_str(self) -> &'static str {
        self.labels().0
    }
}

#[derive(Deserialize)]
pub struct NewLedger {
    name: String,
    kind: LedgerKind,
    group_id: Option<i64>,
}

#[derive(Serialize)]
pub struct Ledger {
    pub(crate) id: i64,
    name: String,
    kind: String,
    group: Option<String>,
    group_id: Option<i64>,
}

#[derive(Deserialize)]
pub struct NewVoucher {
    pub(crate) request_id: String,
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
    Payment(CashForm),
    Receipt(CashForm),
    Journal { postings: Vec<NewPosting> },
    Contra { postings: Vec<NewPosting> },
}

#[derive(Deserialize)]
#[serde(untagged)]
enum CashForm {
    Compound { postings: Vec<NewPosting> },
    Single(CashVoucher),
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
    #[serde(default)]
    narration: String,
    #[serde(default)]
    allocations: Vec<NewAllocation>,
}

#[derive(Deserialize)]
struct NewAllocation {
    category: String,
    centre: String,
    amount: String,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum PostingSide {
    Debit,
    Credit,
}

impl PostingSide {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Debit => "debit",
            Self::Credit => "credit",
        }
    }

    pub(crate) fn from_db(side: &str) -> Self {
        if side == "debit" {
            Self::Debit
        } else {
            Self::Credit
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Posting {
    pub(crate) ledger_id: i64,
    pub(crate) side: PostingSide,
    pub(crate) amount: i64,
    pub(crate) narration: String,
    pub(crate) allocations: Vec<Allocation>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Allocation {
    pub(crate) category: String,
    pub(crate) centre: String,
    pub(crate) amount: i64,
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Revision {
    pub(crate) code: String,
    pub(crate) type_name: String,
    pub(crate) series: String,
    pub(crate) date: String,
    pub(crate) status: String,
    pub(crate) cash_ledger_id: Option<i64>,
    pub(crate) reference: String,
    pub(crate) narration: String,
    pub(crate) source_revision: Option<i64>,
    pub(crate) postings: Vec<Posting>,
}

pub(crate) struct Audit<'a> {
    pub(crate) actor: &'a str,
    pub(crate) action: &'a str,
    pub(crate) reason: &'a str,
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
    type_name: String,
    series: String,
    status: String,
    revision: i32,
    source_guid: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    postings: Vec<VoucherPosting>,
}

#[derive(Debug, Serialize)]
struct VoucherPosting {
    ledger_id: i64,
    ledger: String,
    side: String,
    amount_paise: String,
    narration: String,
    allocations: Vec<VoucherAllocation>,
}

#[derive(Debug, Serialize)]
struct VoucherAllocation {
    category: String,
    centre: String,
    amount_paise: String,
}

#[derive(Default, Deserialize)]
pub struct RegisterFilter {
    kind: Option<VoucherKind>,
    from: Option<String>,
    to: Option<String>,
    q: Option<String>,
    status: Option<String>,
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

#[derive(Deserialize)]
pub struct VoucherChange {
    expected_revision: i32,
    actor: String,
    reason: String,
    voucher: Option<NewVoucher>,
}

impl NewVoucher {
    /// `payment`, `receipt`, `journal` or `contra`.
    pub fn kind_key(&self) -> &'static str {
        match self.entry {
            VoucherEntry::Payment(_) => "payment",
            VoucherEntry::Receipt(_) => "receipt",
            VoucherEntry::Journal { .. } => "journal",
            VoucherEntry::Contra { .. } => "contra",
        }
    }
}

impl VoucherChange {
    /// The replacement voucher, or `None` when the change cancels the voucher.
    pub fn replacement(&self) -> Option<&NewVoucher> {
        self.voucher.as_ref()
    }

    /// Records the signed-in user instead of a typed operator name.
    pub fn set_actor(&mut self, actor: String) {
        self.actor = actor;
    }
}

impl Voucher {
    pub fn kind(&self) -> &str {
        &self.kind
    }
}

#[derive(Serialize)]
pub struct VoucherEvent {
    revision: i32,
    date: String,
    status: String,
    actor: String,
    action: String,
    reason: String,
    recorded_at: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    postings: Vec<VoucherPosting>,
}

pub struct Store {
    pub(crate) connection: Connection,
}

impl Store {
    /// Opens a book file, applying any pending schema migrations.
    /// Call [`crate::migrate::backup_before_migrating`] first to keep a copy.
    pub fn open(path: &Path) -> Result<Self, crate::migrate::MigrationError> {
        Self::initialize(Connection::open(path)?)
    }

    /// A new, empty book in memory, for tests and previews.
    pub fn open_in_memory() -> Result<Self, crate::migrate::MigrationError> {
        Self::initialize(Connection::open_in_memory()?)
    }

    pub(crate) fn initialize(
        mut connection: Connection,
    ) -> Result<Self, crate::migrate::MigrationError> {
        let tx = connection.transaction()?;
        crate::migrate::run(&tx)?;
        tx.commit()?;
        Ok(Self { connection })
    }

    /// The newest schema migration applied to this book.
    pub fn schema_version(&self) -> Result<i32, StoreError> {
        Ok(self.connection.query_row(
            "SELECT coalesce(max(version), 0) FROM schema_migrations",
            [],
            |row| row.get(0),
        )?)
    }

    /// Writes everything in the write-ahead log into the book file, so a plain file
    /// copy is a complete backup.
    pub fn checkpoint(&self) -> Result<(), StoreError> {
        self.connection.execute_batch("CHECKPOINT")?;
        Ok(())
    }

    /// Another connection to the same open book, for concurrent reads.
    /// It shares the database instance, so it sees every committed write.
    pub fn reader(&self) -> Result<Self, StoreError> {
        Ok(Self {
            connection: self.connection.try_clone()?,
        })
    }

    pub fn ledgers(&self) -> Result<Vec<Ledger>, StoreError> {
        let mut stmt = self.connection.prepare(
            "SELECT l.id, l.name, l.kind, g.name, g.id FROM ledgers l
             LEFT JOIN ledger_groups lg ON lg.ledger_id = l.id
             LEFT JOIN account_groups g ON g.id = lg.group_id ORDER BY l.name_key",
        )?;
        Ok(stmt
            .query_map([], ledger_row)?
            .collect::<duckdb::Result<_>>()?)
    }

    pub fn add_ledger(&mut self, entry: NewLedger) -> Result<Ledger, StoreError> {
        let name = required_text(&entry.name, "Ledger name", 100)?;
        let kind = entry.kind.as_str();
        let tx = self.connection.transaction()?;
        let group = entry
            .group_id
            .map(|group_id| {
                tx.query_row(
                    "SELECT name FROM account_groups WHERE id = ?",
                    [group_id],
                    |row| row.get(0),
                )
                .optional()?
                .ok_or_else(|| StoreError::Invalid("Choose an existing group.".into()))
            })
            .transpose()?;
        let id = insert_ledger(&tx, name, kind)?;
        if let Some(group_id) = entry.group_id {
            tx.execute(
                "INSERT INTO ledger_groups VALUES (?, ?)",
                params![id, group_id],
            )?;
        }
        tx.commit()?;
        Ok(Ledger {
            id,
            name: name.into(),
            kind: kind.into(),
            group,
            group_id: entry.group_id,
        })
    }

    pub fn voucher(&self, id: i64) -> Result<Voucher, StoreError> {
        read_voucher(&self.connection, id)
    }

    pub fn history(&self, id: i64) -> Result<Vec<VoucherEvent>, StoreError> {
        let mut stmt = self.connection.prepare(
            "SELECT revision, voucher_date::VARCHAR, status, actor, action, reason, recorded_at::VARCHAR
             FROM voucher_revisions WHERE voucher_id = ? ORDER BY revision DESC"
        )?;
        let events = stmt
            .query_map([id], |row| {
                Ok(VoucherEvent {
                    revision: row.get(0)?,
                    date: row.get(1)?,
                    status: row.get(2)?,
                    actor: row.get(3)?,
                    action: row.get(4)?,
                    reason: row.get(5)?,
                    recorded_at: row.get(6)?,
                    postings: vec![],
                })
            })?
            .collect::<duckdb::Result<Vec<_>>>()?;
        if events.is_empty() {
            return Err(StoreError::Missing("Voucher not found.".into()));
        }
        Ok(events)
    }

    pub fn history_revision(&self, id: i64, revision: i32) -> Result<VoucherEvent, StoreError> {
        let mut event = self
            .history(id)?
            .into_iter()
            .find(|e| e.revision == revision)
            .ok_or_else(|| StoreError::Missing("Revision not found.".into()))?;
        event.postings = display_postings(&self.connection, id, revision)?;
        Ok(event)
    }

    pub fn add_voucher(&mut self, entry: NewVoucher) -> Result<(Voucher, bool), StoreError> {
        self.add_voucher_by(entry, "Local operator")
    }

    /// Posts a voucher and records `actor` as the person who posted it.
    pub fn add_voucher_by(
        &mut self,
        entry: NewVoucher,
        actor: &str,
    ) -> Result<(Voucher, bool), StoreError> {
        let key = request_key(&entry.request_id)?;
        let tx = self.connection.transaction()?;
        let (kind, mut revision) = prepare_local(&tx, entry)?;
        let (kind_key, _, prefix) = kind.labels();
        let existing = find_request(&tx, &key)?;
        if let Some((id, version, saved_kind)) = existing {
            let saved = read_revision(&tx, id, version)?;
            revision.code.clone_from(&saved.code);
            if saved_kind != kind_key || saved != revision {
                return Err(StoreError::Conflict(
                    "This request ID belongs to a different voucher.".into(),
                ));
            }
            return Ok((read_voucher(&tx, id)?, false));
        }

        ensure_open(&tx, &revision.date)?;
        let (id, number): (i64, i64) = tx.query_row(
            "INSERT INTO book_vouchers (kind, local_number)
             SELECT ?, COALESCE(MAX(local_number), 0) + 1 FROM book_vouchers WHERE kind = ?
             RETURNING id, local_number",
            params![kind_key, kind_key],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        revision.code = format!("{prefix}-{number:06}");
        save_revision(
            &tx,
            id,
            1,
            Some(&key),
            &revision,
            Audit {
                actor,
                action: "posted",
                reason: "",
            },
        )?;
        let voucher = read_voucher(&tx, id)?;
        tx.commit()?;
        Ok((voucher, true))
    }

    pub fn change_voucher(
        &mut self,
        id: i64,
        change: VoucherChange,
    ) -> Result<Voucher, StoreError> {
        let actor = required_text(&change.actor, "Operator name", 100)?;
        let reason = required_text(&change.reason, "Reason", 500)?;
        let tx = self.connection.transaction()?;
        let current = read_voucher(&tx, id)?;
        if current.revision != change.expected_revision || current.status != "posted" {
            return Err(StoreError::Conflict(
                "The voucher has changed. Reopen it and check its current status.".into(),
            ));
        }
        if current.source_guid.is_some() {
            return Err(StoreError::Conflict(
                "Import the updated source voucher to revise an imported entry.".into(),
            ));
        }
        ensure_open(&tx, &current.date)?;
        ensure_unmatched(&tx, id, current.revision)?;
        let mut revision = read_revision(&tx, id, current.revision)?;
        let (key, action) = if let Some(entry) = change.voucher {
            let key = request_key(&entry.request_id)?;
            if find_request(&tx, &key)?.is_some() {
                return Err(StoreError::Conflict(
                    "This change has already been submitted.".into(),
                ));
            }
            let (kind, mut updated) = prepare_local(&tx, entry)?;
            if kind.as_str() != current.kind {
                return Err(StoreError::Invalid(
                    "Keep the same voucher type when editing.".into(),
                ));
            }
            ensure_open(&tx, &updated.date)?;
            updated.code = revision.code;
            revision = updated;
            (Some(key), "edited")
        } else {
            revision.status = "cancelled".into();
            (None, "cancelled")
        };
        save_revision(
            &tx,
            id,
            current.revision + 1,
            key.as_deref(),
            &revision,
            Audit {
                actor,
                action,
                reason,
            },
        )?;
        let voucher = read_voucher(&tx, id)?;
        tx.commit()?;
        Ok(voucher)
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
        let status = filter.status.as_deref();
        if status.is_some_and(|value| !matches!(value, "posted" | "cancelled" | "optional")) {
            return Err(StoreError::Invalid("Choose a valid voucher status.".into()));
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
            &format!("SELECT COUNT(*),
                    COALESCE(SUM(amount_paise) FILTER (WHERE kind = 'receipt' AND status = 'posted'), 0)::VARCHAR,
                    COALESCE(SUM(amount_paise) FILTER (WHERE kind = 'payment' AND status = 'posted'), 0)::VARCHAR
                 FROM book_register {REGISTER_FILTER}"),
            params![kind, from, to, q, status],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        let mut stmt = self.connection.prepare(&format!(
            "SELECT {VOUCHER_COLUMNS} FROM book_register {REGISTER_FILTER}
             ORDER BY voucher_date DESC, id DESC LIMIT $6 OFFSET $7"
        ))?;
        let rows = stmt
            .query_map(
                params![kind, from, to, q, status, PAGE_SIZE, offset],
                voucher_row,
            )?
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

fn request_key(value: &str) -> Result<String, StoreError> {
    Uuid::parse_str(value)
        .map(|key| key.to_string())
        .map_err(|_| StoreError::Invalid("Invalid voucher request ID.".into()))
}

fn find_request(connection: &Connection, key: &str) -> duckdb::Result<Option<(i64, i32, String)>> {
    connection
        .query_row(
            "SELECT r.voucher_id, r.revision, v.kind FROM voucher_revisions r
         JOIN book_vouchers v ON v.id = r.voucher_id WHERE r.request_key = ?",
            [key],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
}

pub(crate) fn ensure_open(connection: &Connection, date: &str) -> Result<(), StoreError> {
    let closed: bool = connection.query_row(
        "SELECT COALESCE(?::DATE <= closed_through, false) FROM book_settings WHERE id = 1",
        [date],
        |row| row.get(0),
    )?;
    if closed {
        return Err(StoreError::Conflict(
            "This accounting period is closed.".into(),
        ));
    }
    Ok(())
}

pub(crate) fn ensure_unmatched(
    connection: &Connection,
    id: i64,
    revision: i32,
) -> Result<(), StoreError> {
    let matched: bool = connection.query_row(
        "SELECT EXISTS (SELECT 1 FROM bank_match_postings p JOIN active_bank_matches m
          ON m.id = p.match_id WHERE p.voucher_id = ? AND p.revision = ?)",
        params![id, revision],
        |row| row.get(0),
    )?;
    if matched {
        return Err(StoreError::Conflict(
            "Undo the bank match before changing this voucher.".into(),
        ));
    }
    Ok(())
}

fn prepare_local(
    connection: &Connection,
    entry: NewVoucher,
) -> Result<(VoucherKind, Revision), StoreError> {
    let (kind, cash, compound) = match entry.entry {
        VoucherEntry::Payment(CashForm::Single(form)) => (VoucherKind::Payment, Some(form), vec![]),
        VoucherEntry::Receipt(CashForm::Single(form)) => (VoucherKind::Receipt, Some(form), vec![]),
        VoucherEntry::Payment(CashForm::Compound { postings }) => {
            (VoucherKind::Payment, None, postings)
        }
        VoucherEntry::Receipt(CashForm::Compound { postings }) => {
            (VoucherKind::Receipt, None, postings)
        }
        VoucherEntry::Journal { postings } => (VoucherKind::Journal, None, postings),
        VoucherEntry::Contra { postings } => (VoucherKind::Contra, None, postings),
    };
    let (cash_ledger_id, postings) = if let Some(cash) = cash {
        if !is_liquid(ledger_kind(connection, cash.cash_ledger_id)?.as_deref()) {
            return Err(StoreError::Invalid("Choose a cash or bank ledger.".into()));
        }
        if ledger_kind(connection, cash.other_ledger_id)?.as_deref() != Some("general") {
            return Err(StoreError::Invalid(
                "Choose a party, expense, income, or other general ledger.".into(),
            ));
        }
        let amount = parse_amount(&cash.amount)?;
        let (debit, credit) = if matches!(kind, VoucherKind::Payment) {
            (cash.other_ledger_id, cash.cash_ledger_id)
        } else {
            (cash.cash_ledger_id, cash.other_ledger_id)
        };
        (
            Some(cash.cash_ledger_id),
            vec![
                Posting {
                    ledger_id: debit,
                    side: PostingSide::Debit,
                    amount,
                    narration: String::new(),
                    allocations: vec![],
                },
                Posting {
                    ledger_id: credit,
                    side: PostingSide::Credit,
                    amount,
                    narration: String::new(),
                    allocations: vec![],
                },
            ],
        )
    } else {
        prepare_compound(connection, kind, compound)?
    };
    Ok((
        kind,
        Revision {
            code: String::new(),
            type_name: kind.labels().1.into(),
            series: "Default".into(),
            date: validated_date(&entry.date)?.into(),
            status: "posted".into(),
            cash_ledger_id,
            reference: validated_text(&entry.reference, "Reference", 100)?.into(),
            narration: validated_text(&entry.narration, "Narration", 500)?.into(),
            source_revision: None,
            postings,
        },
    ))
}

fn is_liquid(kind: Option<&str>) -> bool {
    matches!(kind, Some("cash" | "bank"))
}

fn prepare_compound(
    connection: &Connection,
    kind: VoucherKind,
    entries: Vec<NewPosting>,
) -> Result<(Option<i64>, Vec<Posting>), StoreError> {
    if !(2..=MAX_POSTING_LINES).contains(&entries.len()) {
        return Err(StoreError::Invalid(format!(
            "Enter 2 to {MAX_POSTING_LINES} posting lines."
        )));
    }
    let mut liquid = HashSet::new();
    let mut debit_ledgers = HashSet::new();
    let mut credit_ledgers = HashSet::new();
    let mut postings = Vec::with_capacity(entries.len());
    for (index, entry) in entries.into_iter().enumerate() {
        let ledger = ledger_kind(connection, entry.ledger_id)?.ok_or_else(|| {
            StoreError::Invalid(format!("Choose an existing ledger on line {}.", index + 1))
        })?;
        let cash = is_liquid(Some(&ledger));
        match (kind, entry.side, cash) {
            (VoucherKind::Contra, _, false) => {
                return Err(StoreError::Invalid(
                    "Contra vouchers use cash and bank accounts.".into(),
                ));
            }
            (VoucherKind::Payment, PostingSide::Credit, true)
            | (VoucherKind::Receipt, PostingSide::Debit, true) => {
                liquid.insert(entry.ledger_id);
            }
            _ => {}
        }
        match entry.side {
            PostingSide::Debit => debit_ledgers.insert(entry.ledger_id),
            PostingSide::Credit => credit_ledgers.insert(entry.ledger_id),
        };
        let allocations = entry
            .allocations
            .into_iter()
            .map(|a| {
                Ok(Allocation {
                    category: required_text(&a.category, "Cost category", 100)?.into(),
                    centre: required_text(&a.centre, "Cost centre", 100)?.into(),
                    amount: parse_amount(&a.amount)?,
                })
            })
            .collect::<Result<Vec<_>, StoreError>>()?;
        postings.push(Posting {
            ledger_id: entry.ledger_id,
            side: entry.side,
            amount: parse_amount(&entry.amount)?,
            narration: validated_text(&entry.narration, "Line narration", 500)?.into(),
            allocations,
        });
    }
    if matches!(kind, VoucherKind::Contra) && !debit_ledgers.is_disjoint(&credit_ledgers) {
        return Err(StoreError::Invalid(
            "Choose distinct source and destination accounts for a contra voucher.".into(),
        ));
    }
    if matches!(kind, VoucherKind::Payment | VoucherKind::Receipt) && liquid.is_empty() {
        return Err(StoreError::Invalid(
            "The payment or receipt needs a cash or bank posting on the appropriate side.".into(),
        ));
    }
    Ok((
        (liquid.len() == 1).then(|| *liquid.iter().next().unwrap()),
        postings,
    ))
}

pub(crate) fn validate_postings(postings: &[Posting], status: &str) -> Result<i128, StoreError> {
    if postings.is_empty() && status == "cancelled" {
        return Ok(0);
    }
    if postings.len() < 2 {
        return Err(StoreError::Invalid(
            "Enter at least two posting lines.".into(),
        ));
    }
    let mut debit = 0i128;
    let mut credit = 0i128;
    for line in postings {
        match line.side {
            PostingSide::Debit => debit += i128::from(line.amount),
            PostingSide::Credit => credit += i128::from(line.amount),
        }
        let mut categories = std::collections::HashMap::new();
        for allocation in &line.allocations {
            *categories
                .entry(allocation.category.to_lowercase())
                .or_insert(0i128) += i128::from(allocation.amount);
        }
        if categories
            .values()
            .any(|&amount| amount > i128::from(line.amount))
        {
            return Err(StoreError::Invalid(
                "A cost category allocates more than its posting amount.".into(),
            ));
        }
    }
    if debit == 0 || debit != credit {
        return Err(StoreError::Invalid(
            "Voucher debits and credits must match.".into(),
        ));
    }
    Ok(debit)
}

pub(crate) fn save_revision(
    tx: &Transaction<'_>,
    id: i64,
    version: i32,
    key: Option<&str>,
    doc: &Revision,
    audit: Audit<'_>,
) -> Result<(), StoreError> {
    let total = validate_postings(&doc.postings, &doc.status)?;
    tx.execute(
        "INSERT INTO voucher_revisions
         (voucher_id, revision, request_key, source_revision, code, type_name, series, voucher_date,
          status, cash_ledger_id, amount_paise, reference, narration, actor, action, reason)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?::DATE, ?, ?, ?::HUGEINT, ?, ?, ?, ?, ?)",
        params![
            id,
            version,
            key,
            doc.source_revision,
            doc.code,
            doc.type_name,
            doc.series,
            doc.date,
            doc.status,
            doc.cash_ledger_id,
            total.to_string(),
            doc.reference,
            doc.narration,
            audit.actor,
            audit.action,
            audit.reason
        ],
    )?;
    let mut insert = tx.prepare(
        "INSERT INTO posting_lines (voucher_id, revision, line_no, ledger_id, side, amount_paise, narration)
         VALUES (?, ?, ?, ?, ?, ?, ?)"
    )?;
    let mut allocate = tx.prepare("INSERT INTO cost_allocations VALUES (?, ?, ?, ?, ?, ?, ?)")?;
    for (number, posting) in (1i32..).zip(&doc.postings) {
        insert.execute(params![
            id,
            version,
            number,
            posting.ledger_id,
            posting.side.as_str(),
            posting.amount,
            posting.narration
        ])?;
        for (order, allocation) in (1i32..).zip(&posting.allocations) {
            allocate.execute(params![
                id,
                version,
                number,
                order,
                allocation.category,
                allocation.centre,
                allocation.amount
            ])?;
        }
    }
    Ok(())
}

pub(crate) fn read_revision(
    connection: &Connection,
    id: i64,
    version: i32,
) -> duckdb::Result<Revision> {
    let mut doc = connection.query_row(
        "SELECT code, type_name, series, voucher_date::VARCHAR, status, cash_ledger_id, reference,
                narration, source_revision
         FROM voucher_revisions WHERE voucher_id = ? AND revision = ?",
        params![id, version],
        |row| {
            Ok(Revision {
                code: row.get(0)?,
                type_name: row.get(1)?,
                series: row.get(2)?,
                date: row.get(3)?,
                status: row.get(4)?,
                cash_ledger_id: row.get(5)?,
                reference: row.get(6)?,
                narration: row.get(7)?,
                source_revision: row.get(8)?,
                postings: vec![],
            })
        },
    )?;
    let mut stmt = connection.prepare(
        "SELECT ledger_id, side, amount_paise, narration FROM posting_lines
         WHERE voucher_id = ? AND revision = ? ORDER BY line_no",
    )?;
    doc.postings = stmt
        .query_map(params![id, version], |row| {
            Ok(Posting {
                ledger_id: row.get(0)?,
                side: PostingSide::from_db(&row.get::<_, String>(1)?),
                amount: row.get(2)?,
                narration: row.get(3)?,
                allocations: vec![],
            })
        })?
        .collect::<duckdb::Result<_>>()?;
    let mut stmt = connection.prepare(
        "SELECT line_no, category, centre, amount_paise FROM cost_allocations
         WHERE voucher_id = ? AND revision = ? ORDER BY line_no, ordinal",
    )?;
    let allocations = stmt.query_map(params![id, version], |row| {
        Ok((
            row.get::<_, i32>(0)?,
            Allocation {
                category: row.get(1)?,
                centre: row.get(2)?,
                amount: row.get(3)?,
            },
        ))
    })?;
    for allocation in allocations {
        let (number, allocation) = allocation?;
        doc.postings[number as usize - 1]
            .allocations
            .push(allocation);
    }
    Ok(doc)
}

fn read_voucher(connection: &Connection, id: i64) -> Result<Voucher, StoreError> {
    let mut voucher = connection
        .query_row(
            &format!("SELECT {VOUCHER_COLUMNS} FROM book_register WHERE id = ?"),
            [id],
            voucher_row,
        )
        .optional()?
        .ok_or_else(|| StoreError::Missing("Voucher not found.".into()))?;
    voucher.postings = display_postings(connection, id, voucher.revision)?;
    Ok(voucher)
}

fn display_postings(
    connection: &Connection,
    id: i64,
    revision: i32,
) -> duckdb::Result<Vec<VoucherPosting>> {
    let doc = read_revision(connection, id, revision)?;
    let mut names = connection.prepare("SELECT name FROM ledgers WHERE id = ?")?;
    doc.postings
        .into_iter()
        .map(|posting| {
            Ok(VoucherPosting {
                ledger_id: posting.ledger_id,
                ledger: names.query_row([posting.ledger_id], |row| row.get(0))?,
                side: posting.side.as_str().into(),
                amount_paise: posting.amount.to_string(),
                narration: posting.narration,
                allocations: posting
                    .allocations
                    .into_iter()
                    .map(|a| VoucherAllocation {
                        category: a.category,
                        centre: a.centre,
                        amount_paise: a.amount.to_string(),
                    })
                    .collect(),
            })
        })
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
        type_name: row.get(11)?,
        series: row.get(12)?,
        status: row.get(13)?,
        revision: row.get(14)?,
        source_guid: row.get(15)?,
        postings: vec![],
    })
}

fn ledger_row(row: &Row<'_>) -> duckdb::Result<Ledger> {
    Ok(Ledger {
        id: row.get(0)?,
        name: row.get(1)?,
        kind: row.get(2)?,
        group: row.get(3)?,
        group_id: row.get(4)?,
    })
}

pub(crate) fn insert_ledger(
    connection: &Connection,
    name: &str,
    kind: &str,
) -> Result<i64, StoreError> {
    connection
        .query_row(
            "INSERT INTO ledgers (name, name_key, kind) VALUES (?, ?, ?)
         ON CONFLICT (name_key) DO NOTHING RETURNING id",
            params![name, name.to_lowercase(), kind],
            |row| row.get(0),
        )
        .optional()?
        .ok_or_else(|| StoreError::Conflict("A ledger with that name already exists.".into()))
}

pub(crate) fn required_text<'a>(
    text: &'a str,
    label: &str,
    max: usize,
) -> Result<&'a str, StoreError> {
    let text = validated_text(text, label, max)?;
    if text.is_empty() {
        return Err(StoreError::Invalid(format!("Enter {label}.")));
    }
    Ok(text)
}

pub(crate) fn parse_signed_amount(text: &str) -> Result<i64, StoreError> {
    let text = text.trim();
    let unsigned = text.strip_prefix(['-', '+']).unwrap_or(text);
    let value = if matches!(unsigned, "0" | "0.0" | "0.00") {
        0
    } else {
        parse_amount(unsigned)?
    };
    Ok(if text.starts_with('-') { -value } else { value })
}

pub(crate) fn ledger_kind(connection: &Connection, id: i64) -> duckdb::Result<Option<String>> {
    connection
        .query_row("SELECT kind FROM ledgers WHERE id = ?", [id], |row| {
            row.get(0)
        })
        .optional()
}

pub(crate) fn validated_text<'a>(
    text: &'a str,
    label: &str,
    max: usize,
) -> Result<&'a str, StoreError> {
    let text = text.trim();
    if text.chars().count() > max || text.contains('\0') {
        return Err(StoreError::Invalid(format!(
            "{label} must contain at most {max} characters and no null characters."
        )));
    }
    Ok(text)
}

pub(crate) fn validated_date(date: &str) -> Result<&str, StoreError> {
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

pub(crate) fn parse_amount(amount: &str) -> Result<i64, StoreError> {
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
        let mut store = Store::initialize(Connection::open_in_memory().unwrap()).unwrap();
        let bank = store
            .add_ledger(NewLedger {
                name: "Bank of India".into(),
                kind: LedgerKind::Bank,
                group_id: None,
            })
            .unwrap();
        let conveyance = store
            .add_ledger(NewLedger {
                name: "Conveyance".into(),
                kind: LedgerKind::General,
                group_id: None,
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
            entry: VoucherEntry::Payment(CashForm::Single(CashVoucher {
                cash_ledger_id: bank,
                other_ledger_id: conveyance,
                amount: amount.into(),
            })),
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
                        narration: String::new(),
                        allocations: vec![],
                    },
                    NewPosting {
                        ledger_id: cash.id,
                        side: PostingSide::Debit,
                        amount: "0.75".into(),
                        narration: String::new(),
                        allocations: vec![],
                    },
                    NewPosting {
                        ledger_id: credit_ledger,
                        side: PostingSide::Credit,
                        amount: credit.into(),
                        narration: String::new(),
                        allocations: vec![],
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
