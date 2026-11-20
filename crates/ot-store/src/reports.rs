use duckdb::{OptionalExt, params};
use serde::{Deserialize, Serialize};

use crate::store::{
    Store, StoreError, ensure_open, ledger_kind, parse_signed_amount, required_text, validated_date,
};

#[derive(Deserialize)]
pub struct Period {
    pub(crate) from: String,
    pub(crate) to: String,
}

impl Period {
    pub(crate) fn validate(&self) -> Result<(), StoreError> {
        validated_date(&self.from)?;
        validated_date(&self.to)?;
        if self.from > self.to {
            return Err(StoreError::Invalid(
                "The start date must be on or before the end date.".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Serialize)]
pub struct AccountGroup {
    id: i64,
    name: String,
    parent_id: Option<i64>,
}

impl AccountGroup {
    pub fn id(&self) -> i64 {
        self.id
    }
}

#[derive(Deserialize)]
pub struct NewGroup {
    name: String,
    parent_id: Option<i64>,
}

#[derive(Serialize)]
pub struct BookSettings {
    company_name: String,
    source_company: Option<String>,
    closed_through: Option<String>,
}

#[derive(Deserialize)]
pub struct CompanyName {
    name: String,
}

#[derive(Deserialize)]
pub struct ClosePeriod {
    through: String,
    actor: String,
    reason: String,
}

#[derive(Deserialize)]
pub struct Opening {
    date: String,
    amount: String,
    source: String,
}

#[derive(Serialize)]
pub struct TrialRow {
    ledger_id: i64,
    ledger: String,
    group: Option<String>,
    opening_paise: Option<String>,
    debit_paise: String,
    credit_paise: String,
    closing_paise: Option<String>,
}

#[derive(Serialize)]
pub struct TrialBalance {
    rows: Vec<TrialRow>,
    debit_paise: String,
    credit_paise: String,
    opening_difference_paise: Option<String>,
    closing_debit_paise: Option<String>,
    closing_credit_paise: Option<String>,
    missing_openings: usize,
}

#[derive(Serialize)]
pub struct Statement {
    totals: TrialRow,
    rows: Vec<StatementLine>,
}

#[derive(Serialize)]
pub struct StatementLine {
    voucher_id: i64,
    line_no: i32,
    code: String,
    date: String,
    side: String,
    amount_paise: String,
    narration: String,
}

#[derive(Serialize)]
pub struct CostTotal {
    category: String,
    centre: String,
    debit_paise: String,
    credit_paise: String,
}

#[derive(Deserialize)]
pub struct CostFilter {
    #[serde(flatten)]
    period: Period,
    category: String,
    centre: String,
}

const TRIAL_SQL: &str = "
WITH openings AS (
    SELECT ledger_id, opening_date, signed_paise
    FROM ledger_openings WHERE opening_date <= $1::DATE
    QUALIFY row_number() OVER (PARTITION BY ledger_id ORDER BY opening_date DESC) = 1
),
movement AS (
    SELECT p.ledger_id,
           SUM(CASE WHEN p.side = 'debit' THEN p.amount_paise::HUGEINT ELSE -p.amount_paise::HUGEINT END)
             FILTER (WHERE p.voucher_date >= o.opening_date AND p.voucher_date < $1::DATE) AS previous,
           SUM(p.amount_paise) FILTER (WHERE p.side = 'debit' AND p.voucher_date >= $1::DATE) AS debit,
           SUM(p.amount_paise) FILTER (WHERE p.side = 'credit' AND p.voucher_date >= $1::DATE) AS credit
    FROM active_postings p LEFT JOIN openings o USING (ledger_id)
    WHERE p.voucher_date <= $2::DATE
    GROUP BY p.ledger_id
),
balances AS (
    SELECT l.id, l.name, g.name AS account_group,
           o.signed_paise::HUGEINT + COALESCE(m.previous, 0) AS opening,
           COALESCE(m.debit, 0) AS debit, COALESCE(m.credit, 0) AS credit
    FROM ledgers l LEFT JOIN openings o ON l.id = o.ledger_id
    LEFT JOIN movement m ON l.id = m.ledger_id
    LEFT JOIN ledger_groups lg ON l.id = lg.ledger_id LEFT JOIN account_groups g ON g.id = lg.group_id
    WHERE ($3::BIGINT IS NULL OR l.id = $3)
)
SELECT id, name, account_group, opening::VARCHAR, debit::VARCHAR, credit::VARCHAR,
       (opening + debit - credit)::VARCHAR
FROM balances ORDER BY lower(name), id";

impl Store {
    pub fn groups(&self) -> Result<Vec<AccountGroup>, StoreError> {
        let mut stmt = self
            .connection
            .prepare("SELECT id, name, parent_id FROM account_groups ORDER BY name_key")?;
        Ok(stmt
            .query_map([], |r| {
                Ok(AccountGroup {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    parent_id: r.get(2)?,
                })
            })?
            .collect::<duckdb::Result<_>>()?)
    }

    pub fn add_group(&self, group: NewGroup) -> Result<AccountGroup, StoreError> {
        let name = required_text(&group.name, "Group name", 100)?;
        if let Some(parent_id) = group.parent_id {
            let parent_exists: bool = self.connection.query_row(
                "SELECT EXISTS (SELECT 1 FROM account_groups WHERE id = ?)",
                [parent_id],
                |r| r.get(0),
            )?;
            if !parent_exists {
                return Err(StoreError::Invalid(
                    "Choose an existing parent group.".into(),
                ));
            }
        }
        let id = self
            .connection
            .query_row(
                "INSERT INTO account_groups (name, name_key, parent_id) VALUES (?, ?, ?)
             ON CONFLICT (name_key) DO NOTHING RETURNING id",
                params![name, name.to_lowercase(), group.parent_id],
                |r| r.get(0),
            )
            .optional()?
            .ok_or_else(|| {
                StoreError::Conflict("An account group with that name already exists.".into())
            })?;
        Ok(AccountGroup {
            id,
            name: name.into(),
            parent_id: group.parent_id,
        })
    }

    pub fn settings(&self) -> Result<BookSettings, StoreError> {
        Ok(self.connection.query_row(
            "SELECT company_name, source_company, closed_through::VARCHAR FROM book_settings WHERE id = 1",
            [], |r| Ok(BookSettings { company_name: r.get(0)?, source_company: r.get(1)?, closed_through: r.get(2)? }),
        )?)
    }

    pub fn name_company(&self, company: CompanyName) -> Result<BookSettings, StoreError> {
        let name = required_text(&company.name, "Company name", 150)?;
        self.connection.execute(
            "UPDATE book_settings SET company_name = ? WHERE id = 1",
            [name],
        )?;
        self.settings()
    }

    pub fn close_period(&mut self, request: ClosePeriod) -> Result<BookSettings, StoreError> {
        let through = validated_date(&request.through)?;
        let actor = required_text(&request.actor, "Operator name", 100)?;
        let reason = required_text(&request.reason, "Reason", 500)?;
        let tx = self.connection.transaction()?;
        ensure_open(&tx, through)?;
        tx.execute(
            "INSERT INTO period_closures (closed_through, actor, reason) VALUES (?::DATE, ?, ?)",
            params![through, actor, reason],
        )?;
        tx.execute(
            "UPDATE book_settings SET closed_through = ?::DATE WHERE id = 1",
            [through],
        )?;
        tx.commit()?;
        self.settings()
    }

    pub fn record_opening(&self, ledger_id: i64, entry: Opening) -> Result<(), StoreError> {
        let date = validated_date(&entry.date)?;
        let amount = parse_signed_amount(&entry.amount)?;
        let source = required_text(&entry.source, "Opening balance source", 200)?;
        ensure_open(&self.connection, date)?;
        if ledger_kind(&self.connection, ledger_id)?.is_none() {
            return Err(StoreError::Missing("Ledger not found.".into()));
        }
        let existing = self.connection.query_row(
            "SELECT signed_paise, source FROM ledger_openings WHERE ledger_id = ? AND opening_date = ?::DATE",
            params![ledger_id, date], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)),
        ).optional()?;
        if let Some((saved_amount, saved_source)) = existing {
            if saved_amount == amount && saved_source == source {
                return Ok(());
            }
            return Err(StoreError::Conflict(
                "An opening balance already exists on this date.".into(),
            ));
        }
        self.connection.execute(
            "INSERT INTO ledger_openings VALUES (?, ?::DATE, ?, ?)",
            params![ledger_id, date, amount, source],
        )?;
        Ok(())
    }

    pub fn trial_balance(&self, period: Period) -> Result<TrialBalance, StoreError> {
        let rows = self.trial_rows(&period, None)?;
        let mut debits = 0i128;
        let mut credits = 0i128;
        let mut opening_difference = 0i128;
        let mut closing_debits = 0i128;
        let mut closing_credits = 0i128;
        let mut missing = 0;
        for row in &rows {
            debits += report_amount(&row.debit_paise)?;
            credits += report_amount(&row.credit_paise)?;
            if let (Some(opening), Some(closing)) = (&row.opening_paise, &row.closing_paise) {
                opening_difference += report_amount(opening)?;
                let closing = report_amount(closing)?;
                closing_debits += closing.max(0);
                closing_credits -= closing.min(0);
            } else {
                missing += 1;
            }
        }
        Ok(TrialBalance {
            rows,
            debit_paise: debits.to_string(),
            credit_paise: credits.to_string(),
            opening_difference_paise: (missing == 0).then(|| opening_difference.to_string()),
            closing_debit_paise: (missing == 0).then(|| closing_debits.to_string()),
            closing_credit_paise: (missing == 0).then(|| closing_credits.to_string()),
            missing_openings: missing,
        })
    }

    pub fn statement(&self, ledger_id: i64, period: Period) -> Result<Statement, StoreError> {
        let totals = self
            .trial_rows(&period, Some(ledger_id))?
            .into_iter()
            .next()
            .ok_or_else(|| StoreError::Missing("Ledger not found.".into()))?;
        let mut stmt = self.connection.prepare(
            "SELECT voucher_id, line_no, code, voucher_date::VARCHAR, side, amount_paise::VARCHAR, narration
             FROM active_postings WHERE ledger_id = ? AND voucher_date BETWEEN ?::DATE AND ?::DATE
             ORDER BY voucher_date, voucher_id, line_no"
        )?;
        let rows = stmt
            .query_map(params![ledger_id, period.from, period.to], |r| {
                Ok(StatementLine {
                    voucher_id: r.get(0)?,
                    line_no: r.get(1)?,
                    code: r.get(2)?,
                    date: r.get(3)?,
                    side: r.get(4)?,
                    amount_paise: r.get(5)?,
                    narration: r.get(6)?,
                })
            })?
            .collect::<duckdb::Result<_>>()?;
        Ok(Statement { totals, rows })
    }

    pub fn cost_totals(&self, period: Period) -> Result<Vec<CostTotal>, StoreError> {
        period.validate()?;
        let mut stmt = self.connection.prepare(
            "SELECT MIN(a.category), MIN(a.centre),
                    COALESCE(SUM(a.amount_paise) FILTER (WHERE p.side = 'debit'), 0)::VARCHAR,
                    COALESCE(SUM(a.amount_paise) FILTER (WHERE p.side = 'credit'), 0)::VARCHAR
             FROM cost_allocations a JOIN active_postings p USING (voucher_id, revision, line_no)
             WHERE p.voucher_date BETWEEN ?::DATE AND ?::DATE GROUP BY lower(a.category), lower(a.centre)
             ORDER BY 1, 2"
        )?;
        Ok(stmt
            .query_map(params![period.from, period.to], |r| {
                Ok(CostTotal {
                    category: r.get(0)?,
                    centre: r.get(1)?,
                    debit_paise: r.get(2)?,
                    credit_paise: r.get(3)?,
                })
            })?
            .collect::<duckdb::Result<_>>()?)
    }

    pub fn cost_postings(&self, filter: CostFilter) -> Result<Vec<StatementLine>, StoreError> {
        filter.period.validate()?;
        let category = required_text(&filter.category, "Cost category", 100)?;
        let centre = required_text(&filter.centre, "Cost centre", 100)?;
        let mut stmt = self.connection.prepare(
            "SELECT p.voucher_id, p.line_no, p.code, p.voucher_date::VARCHAR, p.side,
                    a.amount_paise::VARCHAR, p.narration
             FROM cost_allocations a JOIN active_postings p USING (voucher_id, revision, line_no)
             WHERE p.voucher_date BETWEEN ?::DATE AND ?::DATE
               AND lower(a.category) = lower(?) AND lower(a.centre) = lower(?)
             ORDER BY p.voucher_date, p.voucher_id, p.line_no, a.ordinal",
        )?;
        Ok(stmt
            .query_map(
                params![filter.period.from, filter.period.to, category, centre],
                |r| {
                    Ok(StatementLine {
                        voucher_id: r.get(0)?,
                        line_no: r.get(1)?,
                        code: r.get(2)?,
                        date: r.get(3)?,
                        side: r.get(4)?,
                        amount_paise: r.get(5)?,
                        narration: r.get(6)?,
                    })
                },
            )?
            .collect::<duckdb::Result<_>>()?)
    }

    fn trial_rows(
        &self,
        period: &Period,
        ledger_id: Option<i64>,
    ) -> Result<Vec<TrialRow>, StoreError> {
        period.validate()?;
        let mut stmt = self.connection.prepare(TRIAL_SQL)?;
        Ok(stmt
            .query_map(params![period.from, period.to, ledger_id], |r| {
                Ok(TrialRow {
                    ledger_id: r.get(0)?,
                    ledger: r.get(1)?,
                    group: r.get(2)?,
                    opening_paise: r.get(3)?,
                    debit_paise: r.get(4)?,
                    credit_paise: r.get(5)?,
                    closing_paise: r.get(6)?,
                })
            })?
            .collect::<duckdb::Result<_>>()?)
    }
}

fn report_amount(value: &str) -> Result<i128, StoreError> {
    value.parse().map_err(|_| StoreError::Unavailable)
}
