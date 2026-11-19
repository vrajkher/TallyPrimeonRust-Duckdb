use std::collections::HashSet;

use duckdb::{Connection, OptionalExt, params};
use serde::{Deserialize, Serialize};

use crate::{
    reports::Period,
    store::{
        Store, StoreError, ledger_kind, parse_signed_amount, required_text, validated_date,
        validated_text,
    },
};

#[derive(Deserialize)]
pub struct StatementImport {
    content: String,
}

#[derive(Deserialize)]
struct CsvLine {
    transaction_id: String,
    date: String,
    amount: String,
    #[serde(default)]
    reference: String,
    #[serde(default)]
    description: String,
}

#[derive(Serialize)]
pub struct StatementImportResult {
    inserted: usize,
    duplicates: usize,
}

#[derive(Serialize)]
pub struct Review {
    statement: Vec<StatementRow>,
    book: Vec<BookRow>,
}

#[derive(Serialize)]
pub struct StatementRow {
    id: i64,
    transaction_id: String,
    date: String,
    signed_paise: String,
    reference: String,
    description: String,
    match_id: Option<i64>,
}

#[derive(Clone, Copy, Deserialize, Eq, Hash, PartialEq)]
struct PostingKey {
    voucher_id: i64,
    revision: i32,
    line_no: i32,
}

#[derive(Serialize)]
pub struct BookRow {
    voucher_id: i64,
    revision: i32,
    line_no: i32,
    date: String,
    code: String,
    signed_paise: String,
    match_id: Option<i64>,
}

#[derive(Deserialize)]
pub struct MatchRequest {
    statement_ids: Vec<i64>,
    postings: Vec<PostingKey>,
    actor: String,
    #[serde(default)]
    note: String,
}

#[derive(Serialize)]
pub struct MatchResult {
    match_id: i64,
    signed_paise: String,
}

#[derive(Deserialize)]
pub struct UndoMatch {
    actor: String,
    reason: String,
}

impl Store {
    pub fn import_statement(
        &mut self,
        ledger_id: i64,
        import: StatementImport,
    ) -> Result<StatementImportResult, StoreError> {
        require_bank(&self.connection, ledger_id)?;
        let tx = self.connection.transaction()?;
        let mut reader =
            csv::Reader::from_reader(import.content.trim_start_matches('\u{feff}').as_bytes());
        let mut result = StatementImportResult {
            inserted: 0,
            duplicates: 0,
        };
        for (index, record) in reader.deserialize::<CsvLine>().enumerate() {
            let line = record.map_err(|e| {
                StoreError::Invalid(format!("Statement data row {}. {e}", index + 1))
            })?;
            let identity = required_text(&line.transaction_id, "Transaction ID", 150)?;
            let date = validated_date(&line.date)?;
            let amount = parse_signed_amount(&line.amount)?;
            let reference = validated_text(&line.reference, "Bank reference", 100)?;
            let description = validated_text(&line.description, "Bank description", 500)?;
            if amount == 0 {
                return Err(StoreError::Invalid(format!(
                    "Statement data row {} has a zero amount.",
                    index + 1
                )));
            }

            let existing: Option<bool> = tx.query_row(
                "SELECT statement_date = ?::DATE AND signed_paise = ? AND reference = ? AND description = ?
                 FROM bank_statement_lines WHERE ledger_id = ? AND transaction_id = ?",
                params![date, amount, reference, description, ledger_id, identity], |r| r.get(0),
            ).optional()?;
            match existing {
                Some(true) => result.duplicates += 1,
                Some(false) => {
                    return Err(StoreError::Conflict(format!(
                        "Statement data row {} conflicts with its existing transaction ID.",
                        index + 1
                    )));
                }
                None => {
                    tx.execute("INSERT INTO bank_statement_lines (ledger_id, transaction_id, statement_date, signed_paise, reference, description)
                                VALUES (?, ?, ?::DATE, ?, ?, ?)",
                        params![ledger_id, identity, date, amount, reference, description])?;
                    result.inserted += 1;
                }
            }
        }
        if result.inserted + result.duplicates == 0 {
            return Err(StoreError::Invalid("The CSV has no statement rows.".into()));
        }
        tx.commit()?;
        Ok(result)
    }

    pub fn reconciliation(&self, ledger_id: i64, period: Period) -> Result<Review, StoreError> {
        require_bank(&self.connection, ledger_id)?;
        period.validate()?;
        let mut statements = self.connection.prepare(
            "SELECT s.id, s.transaction_id, s.statement_date::VARCHAR, s.signed_paise::VARCHAR,
                    s.reference, s.description, m.match_id
             FROM bank_statement_lines s LEFT JOIN (
                SELECT b.* FROM bank_match_lines b JOIN active_bank_matches a ON a.id = b.match_id
             ) m ON m.statement_id = s.id
             WHERE s.ledger_id = ? AND s.statement_date BETWEEN ?::DATE AND ?::DATE
             ORDER BY s.statement_date, s.id",
        )?;
        let statement = statements
            .query_map(params![ledger_id, period.from, period.to], |r| {
                Ok(StatementRow {
                    id: r.get(0)?,
                    transaction_id: r.get(1)?,
                    date: r.get(2)?,
                    signed_paise: r.get(3)?,
                    reference: r.get(4)?,
                    description: r.get(5)?,
                    match_id: r.get(6)?,
                })
            })?
            .collect::<duckdb::Result<_>>()?;
        let mut postings = self.connection.prepare(
            "SELECT p.voucher_id, p.revision, p.line_no, p.voucher_date::VARCHAR, p.code,
                    (CASE p.side WHEN 'debit' THEN p.amount_paise ELSE -p.amount_paise END)::VARCHAR, m.match_id
             FROM active_postings p LEFT JOIN (
                SELECT b.* FROM bank_match_postings b JOIN active_bank_matches a ON a.id = b.match_id
             ) m USING (voucher_id, revision, line_no)
             WHERE p.ledger_id = ? AND p.voucher_date BETWEEN ?::DATE AND ?::DATE
             ORDER BY p.voucher_date, p.voucher_id, p.line_no"
        )?;
        let book = postings
            .query_map(params![ledger_id, period.from, period.to], |r| {
                Ok(BookRow {
                    voucher_id: r.get(0)?,
                    revision: r.get(1)?,
                    line_no: r.get(2)?,
                    date: r.get(3)?,
                    code: r.get(4)?,
                    signed_paise: r.get(5)?,
                    match_id: r.get(6)?,
                })
            })?
            .collect::<duckdb::Result<_>>()?;
        Ok(Review { statement, book })
    }

    pub fn reconcile(
        &mut self,
        ledger_id: i64,
        request: MatchRequest,
    ) -> Result<MatchResult, StoreError> {
        require_bank(&self.connection, ledger_id)?;
        let actor = required_text(&request.actor, "Operator name", 100)?;
        let note = validated_text(&request.note, "Match note", 500)?;
        if request.statement_ids.is_empty()
            || request.postings.is_empty()
            || request.statement_ids.len() > 100
            || request.postings.len() > 100
        {
            return Err(StoreError::Invalid(
                "Select between 1 and 100 rows on each side.".into(),
            ));
        }
        let tx = self.connection.transaction()?;
        let statement_total = sum_unmatched(&request.statement_ids, |id| {
            tx.query_row(
                "SELECT s.signed_paise FROM bank_statement_lines s
                 WHERE s.id = ? AND s.ledger_id = ? AND NOT EXISTS (
                    SELECT 1 FROM bank_match_lines b JOIN active_bank_matches a ON a.id = b.match_id
                    WHERE b.statement_id = s.id
                 )",
                params![id, ledger_id],
                |r| r.get(0),
            )
            .optional()
        })?;
        let book_total = sum_unmatched(&request.postings, |p| {
            tx.query_row(
                "SELECT CASE p.side WHEN 'debit' THEN p.amount_paise ELSE -p.amount_paise END FROM active_postings p
                 WHERE p.voucher_id = ? AND p.revision = ? AND p.line_no = ? AND p.ledger_id = ?
                 AND NOT EXISTS (SELECT 1 FROM bank_match_postings b JOIN active_bank_matches a ON a.id = b.match_id
                                 WHERE b.voucher_id = p.voucher_id AND b.revision = p.revision AND b.line_no = p.line_no)",
                params![p.voucher_id, p.revision, p.line_no, ledger_id], |r| r.get(0),
            ).optional()
        })?;
        if statement_total == 0 || statement_total != book_total {
            return Err(StoreError::Invalid(
                "Selected statement and ledger totals must match and be nonzero.".into(),
            ));
        }
        let match_id: i64 = tx.query_row(
            "INSERT INTO bank_matches (ledger_id, actor, note) VALUES (?, ?, ?) RETURNING id",
            params![ledger_id, actor, note],
            |r| r.get(0),
        )?;
        for id in request.statement_ids {
            tx.execute(
                "INSERT INTO bank_match_lines VALUES (?, ?)",
                params![match_id, id],
            )?;
        }
        for p in request.postings {
            tx.execute(
                "INSERT INTO bank_match_postings VALUES (?, ?, ?, ?)",
                params![match_id, p.voucher_id, p.revision, p.line_no],
            )?;
        }
        tx.commit()?;
        Ok(MatchResult {
            match_id,
            signed_paise: statement_total.to_string(),
        })
    }

    pub fn undo_reconciliation(&self, match_id: i64, request: UndoMatch) -> Result<(), StoreError> {
        let actor = required_text(&request.actor, "Operator name", 100)?;
        let reason = required_text(&request.reason, "Reason", 500)?;
        let count = self.connection.execute(
            "INSERT INTO bank_match_undos (match_id, actor, reason)
             SELECT id, ?, ? FROM active_bank_matches WHERE id = ?",
            params![actor, reason, match_id],
        )?;
        if count == 0 {
            return Err(StoreError::Missing("Active bank match not found.".into()));
        }
        Ok(())
    }
}

fn sum_unmatched<T: Eq + std::hash::Hash>(
    rows: &[T],
    mut fetch: impl FnMut(&T) -> duckdb::Result<Option<i64>>,
) -> Result<i128, StoreError> {
    let mut seen = HashSet::new();
    let mut sum = 0i128;
    for row in rows {
        if !seen.insert(row) {
            return Err(StoreError::Invalid("Select each row once.".into()));
        }
        let amount = fetch(row)?.ok_or_else(|| {
            StoreError::Conflict(
                "At least one row was already matched or changed. Refresh the review.".into(),
            )
        })?;
        sum += i128::from(amount);
    }
    Ok(sum)
}

fn require_bank(connection: &Connection, ledger_id: i64) -> Result<(), StoreError> {
    if ledger_kind(connection, ledger_id)?.as_deref() != Some("bank") {
        return Err(StoreError::Invalid(
            "Choose an existing bank ledger.".into(),
        ));
    }
    Ok(())
}
