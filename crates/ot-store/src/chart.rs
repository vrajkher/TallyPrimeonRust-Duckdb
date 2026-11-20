//! The chart of accounts and Frappe Books' accounting settings (roadmap steps 1.1, 1.2).
//!
//! Groups are group accounts and ledgers are ledger accounts. A group's root type is
//! the one set on it, else the nearest ancestor's, else the one Tally's reserved group
//! names imply ("Sundry Debtors" is an Asset). A ledger's account type is its own, else
//! its group's, else Cash or Bank from the ledger's kind.

use std::collections::HashMap;

use duckdb::{OptionalExt, Transaction, params};
use ot_core::{
    accounts::{
        AccountType, RootType, check_type_change, check_type_for_root, load_chart, tally_root_type,
    },
    money::Currency,
};
use serde::{Deserialize, Serialize};

use crate::store::{Store, StoreError, required_text};

/// How a group's root type was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RootSource {
    /// Set on this group.
    Set,
    /// Taken from a parent group.
    Inherited,
    /// Implied by one of Tally's reserved group names.
    TallyName,
}

#[derive(Debug, Clone, Serialize)]
pub struct GroupAccount {
    pub id: i64,
    pub name: String,
    pub parent_id: Option<i64>,
    pub root_type: Option<RootType>,
    pub root_source: Option<RootSource>,
    /// The type set on this group, which its ledgers take unless they have one.
    pub account_type: Option<AccountType>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LedgerAccount {
    pub id: i64,
    pub name: String,
    pub kind: String,
    pub group_id: Option<i64>,
    pub root_type: Option<RootType>,
    /// The type set on this ledger.
    pub account_type: Option<AccountType>,
    /// The type the ledger acts as: its own, its group's, or Cash/Bank from its kind.
    pub effective_type: Option<AccountType>,
    pub gst_head: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChartOfAccounts {
    pub groups: Vec<GroupAccount>,
    pub ledgers: Vec<LedgerAccount>,
    /// Ledgers with no root type yet: ungrouped, or in groups nobody has classified.
    pub unclassified_ledgers: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChartApplied {
    pub chart: String,
    pub groups_created: usize,
    pub ledgers_created: usize,
    pub ledgers_placed: usize,
    pub settings: AccountingSettings,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AccountingSettings {
    pub country: Option<String>,
    pub currency: String,
    pub fiscal_year_start: Option<String>,
    pub fiscal_year_end: Option<String>,
    pub chart: Option<String>,
    pub gstin: Option<String>,
    pub bank_ledger_id: Option<i64>,
    pub round_off_ledger_id: Option<i64>,
    pub write_off_ledger_id: Option<i64>,
    pub discount_ledger_id: Option<i64>,
    pub receivable_ledger_id: Option<i64>,
    pub payable_ledger_id: Option<i64>,
    pub setup_complete: bool,
}

/// Changes to the accounting settings. Absent fields stay as they are; `null` clears an
/// optional one.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettingsUpdate {
    #[serde(default, deserialize_with = "some")]
    pub country: Option<Option<String>>,
    pub currency: Option<String>,
    #[serde(default, deserialize_with = "some")]
    pub fiscal_year_start: Option<Option<String>>,
    #[serde(default, deserialize_with = "some")]
    pub gstin: Option<Option<String>>,
    #[serde(default, deserialize_with = "some")]
    pub round_off_ledger_id: Option<Option<i64>>,
    #[serde(default, deserialize_with = "some")]
    pub write_off_ledger_id: Option<Option<i64>>,
    #[serde(default, deserialize_with = "some")]
    pub discount_ledger_id: Option<Option<i64>>,
    #[serde(default, deserialize_with = "some")]
    pub receivable_ledger_id: Option<Option<i64>>,
    #[serde(default, deserialize_with = "some")]
    pub payable_ledger_id: Option<Option<i64>>,
    pub setup_complete: Option<bool>,
}

/// Tells a present `null` apart from an absent field.
fn some<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

/// What the setup wizard sends. Absent fields take the country's suggestions.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompanySetup {
    pub company_name: String,
    pub country: String,
    pub bank_name: String,
    pub currency: Option<String>,
    pub chart: Option<String>,
    pub fiscal_year_start: Option<String>,
    pub fiscal_year_end: Option<String>,
    pub gstin: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SetupResult {
    pub company_name: String,
    pub chart: ChartApplied,
    pub settings: AccountingSettings,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LedgerAccountUpdate {
    pub account_type: Option<AccountType>,
    #[serde(default)]
    pub gst_head: Option<String>,
}

fn invalid(error: impl std::fmt::Display) -> StoreError {
    StoreError::Invalid(error.to_string())
}

struct RawGroup {
    id: i64,
    name: String,
    parent_id: Option<i64>,
    root_type: Option<RootType>,
    account_type: Option<AccountType>,
}

fn read_groups(connection: &duckdb::Connection) -> Result<Vec<RawGroup>, StoreError> {
    let mut stmt = connection.prepare(
        "SELECT g.id, g.name, g.parent_id, a.root_type, a.account_type
         FROM account_groups g LEFT JOIN group_accounts a ON a.group_id = g.id
         ORDER BY g.name_key",
    )?;
    let rows = stmt
        .query_map([], |row| {
            Ok(RawGroup {
                id: row.get(0)?,
                name: row.get(1)?,
                parent_id: row.get(2)?,
                root_type: row
                    .get::<_, Option<String>>(3)?
                    .as_deref()
                    .and_then(RootType::from_label),
                account_type: row
                    .get::<_, Option<String>>(4)?
                    .as_deref()
                    .and_then(AccountType::from_label),
            })
        })?
        .collect::<duckdb::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Each group's root type and where it came from, walking up to the first group that
/// has one set or carries a Tally reserved name.
fn resolve_groups(raw: Vec<RawGroup>) -> Vec<GroupAccount> {
    let by_id: HashMap<i64, &RawGroup> = raw.iter().map(|g| (g.id, g)).collect();
    let resolve = |start: &RawGroup| -> (Option<RootType>, Option<RootSource>) {
        let mut current = Some(start);
        let mut first = true;
        let mut guard = 0;
        while let Some(group) = current {
            if let Some(root) = group.root_type {
                return (
                    Some(root),
                    Some(if first {
                        RootSource::Set
                    } else {
                        RootSource::Inherited
                    }),
                );
            }
            if let Some(root) = tally_root_type(&group.name) {
                return (
                    Some(root),
                    Some(if first {
                        RootSource::TallyName
                    } else {
                        RootSource::Inherited
                    }),
                );
            }
            first = false;
            guard += 1;
            current = group
                .parent_id
                .and_then(|id| by_id.get(&id).copied())
                .filter(|_| guard < 64);
        }
        (None, None)
    };
    raw.iter()
        .map(|group| {
            let (root_type, root_source) = resolve(group);
            GroupAccount {
                id: group.id,
                name: group.name.clone(),
                parent_id: group.parent_id,
                root_type,
                root_source,
                account_type: group.account_type,
            }
        })
        .collect()
}

/// The account type a ledger in `group_id` inherits from its nearest typed group.
fn group_type(
    groups: &HashMap<i64, &GroupAccount>,
    mut group_id: Option<i64>,
) -> Option<AccountType> {
    for _ in 0..64 {
        let group = groups.get(&group_id?)?;
        if group.account_type.is_some() {
            return group.account_type;
        }
        group_id = group.parent_id;
    }
    None
}

fn kind_type(kind: &str) -> Option<AccountType> {
    match kind {
        "cash" => Some(AccountType::Cash),
        "bank" => Some(AccountType::Bank),
        _ => None,
    }
}

fn chart_of(connection: &duckdb::Connection) -> Result<ChartOfAccounts, StoreError> {
    let groups = resolve_groups(read_groups(connection)?);
    let by_id: HashMap<i64, &GroupAccount> = groups.iter().map(|g| (g.id, g)).collect();
    let mut stmt = connection.prepare(
        "SELECT l.id, l.name, l.kind, lg.group_id, a.account_type, a.gst_head
         FROM ledgers l LEFT JOIN ledger_groups lg ON lg.ledger_id = l.id
         LEFT JOIN ledger_accounts a ON a.ledger_id = l.id ORDER BY l.name_key",
    )?;
    let ledgers: Vec<LedgerAccount> = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<i64>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
            ))
        })?
        .collect::<duckdb::Result<Vec<_>>>()?
        .into_iter()
        .map(|(id, name, kind, group_id, own, gst_head)| {
            let account_type = own.as_deref().and_then(AccountType::from_label);
            let root_type = group_id
                .and_then(|g| by_id.get(&g))
                .and_then(|g| g.root_type);
            LedgerAccount {
                effective_type: account_type
                    .or_else(|| group_type(&by_id, group_id))
                    .or_else(|| kind_type(&kind)),
                id,
                name,
                kind,
                group_id,
                root_type,
                account_type,
                gst_head,
            }
        })
        .collect();
    let unclassified_ledgers = ledgers.iter().filter(|l| l.root_type.is_none()).count();
    Ok(ChartOfAccounts {
        groups,
        ledgers,
        unclassified_ledgers,
    })
}

fn settings_of(connection: &duckdb::Connection) -> Result<AccountingSettings, StoreError> {
    Ok(connection.query_row(
        "SELECT country, currency, CAST(fiscal_year_start AS VARCHAR), chart, gstin,
                round_off_ledger_id, write_off_ledger_id, discount_ledger_id,
                receivable_ledger_id, payable_ledger_id, setup_complete,
                CAST(fiscal_year_end AS VARCHAR), bank_ledger_id
         FROM accounting_settings WHERE id = 1",
        [],
        |row| {
            Ok(AccountingSettings {
                country: row.get(0)?,
                currency: row.get(1)?,
                fiscal_year_start: row.get(2)?,
                chart: row.get(3)?,
                gstin: row.get(4)?,
                round_off_ledger_id: row.get(5)?,
                write_off_ledger_id: row.get(6)?,
                discount_ledger_id: row.get(7)?,
                receivable_ledger_id: row.get(8)?,
                payable_ledger_id: row.get(9)?,
                setup_complete: row.get(10)?,
                fiscal_year_end: row.get(11)?,
                bank_ledger_id: row.get(12)?,
            })
        },
    )?)
}

fn find_group(tx: &Transaction<'_>, name: &str) -> Result<Option<i64>, StoreError> {
    Ok(tx
        .query_row(
            "SELECT id FROM account_groups WHERE name_key = ?",
            [name.to_lowercase()],
            |row| row.get(0),
        )
        .optional()?)
}

fn find_ledger(tx: &Transaction<'_>, name: &str) -> Result<Option<(i64, Option<i64>)>, StoreError> {
    Ok(tx
        .query_row(
            "SELECT l.id, lg.group_id FROM ledgers l LEFT JOIN ledger_groups lg ON lg.ledger_id = l.id
             WHERE l.name_key = ?",
            [name.to_lowercase()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?)
}

impl Store {
    pub fn chart_of_accounts(&self) -> Result<ChartOfAccounts, StoreError> {
        chart_of(&self.connection)
    }

    pub fn accounting_settings(&self) -> Result<AccountingSettings, StoreError> {
        settings_of(&self.connection)
    }

    /// Adds a chart template's groups and ledgers to the book. Existing groups and
    /// ledgers with the same names are kept and classified; ungrouped ledgers are
    /// placed in the chart's group. Settings defaults that are still empty are filled.
    pub fn apply_chart(&mut self, chart: &str) -> Result<ChartApplied, StoreError> {
        let tx = self.connection.transaction()?;
        let applied = apply_chart_in(&tx, chart)?;
        tx.commit()?;
        Ok(applied)
    }

    /// Sets up the company once, as Frappe Books' setup wizard does: names it, applies
    /// the chart, adds the company's bank ledger and a Discounts ledger, records the
    /// country, currency, fiscal year and GSTIN, fills the default accounts and marks
    /// setup complete. Fields left out take the country's suggestions.
    pub fn setup_company(
        &mut self,
        request: CompanySetup,
        today: chrono::NaiveDate,
    ) -> Result<SetupResult, StoreError> {
        use ot_core::setup::{country_defaults, parse_date, validate_fiscal_year};
        if settings_of(&self.connection)?.setup_complete {
            return Err(StoreError::Conflict(
                "The company is already set up.".into(),
            ));
        }
        let company = required_text(&request.company_name, "Company name", 150)?.to_owned();
        let bank_name = required_text(&request.bank_name, "Bank name", 100)?.to_owned();
        let defaults = country_defaults(request.country.trim(), today).map_err(invalid)?;
        let currency = Currency::new(
            request
                .currency
                .as_deref()
                .or(defaults.currency.as_deref())
                .unwrap_or("INR"),
        )
        .map_err(invalid)?
        .code()
        .to_owned();
        let chart = request.chart.clone().unwrap_or(defaults.chart.clone());
        let start = parse_date(
            request
                .fiscal_year_start
                .as_deref()
                .unwrap_or(&defaults.fiscal_year_start),
        )
        .map_err(invalid)?;
        let end = match request.fiscal_year_end.as_deref() {
            Some(text) => parse_date(text).map_err(invalid)?,
            None if request.fiscal_year_start.is_some() => ot_core::setup::fiscal_year_end(start),
            None => parse_date(&defaults.fiscal_year_end).map_err(invalid)?,
        };
        validate_fiscal_year(start, end).map_err(invalid)?;
        let gstin = request
            .gstin
            .as_deref()
            .map(|g| g.trim().to_uppercase())
            .filter(|g| !g.is_empty());
        if let Some(g) = &gstin
            && (g.len() != 15 || !g.chars().all(|c| c.is_ascii_alphanumeric()))
        {
            return Err(StoreError::Invalid(
                "A GSTIN has 15 letters and digits.".into(),
            ));
        }

        let tx = self.connection.transaction()?;
        tx.execute(
            "UPDATE book_settings SET company_name = ? WHERE id = 1",
            [&company],
        )?;
        let applied = apply_chart_in(&tx, &chart)?;
        let bank_ledger_id = ensure_ledger(&tx, &bank_name, AccountType::Bank)?;
        ensure_ledger(&tx, "Discounts", AccountType::IncomeAccount)?;
        tx.execute(
            "UPDATE accounting_settings SET country = ?, currency = ?, fiscal_year_start = ?,
                    fiscal_year_end = ?, gstin = ?, bank_ledger_id = ?, chart = ?, setup_complete = TRUE
             WHERE id = 1",
            params![defaults.country, currency, start.to_string(), end.to_string(), gstin, bank_ledger_id, chart],
        )?;
        fill_default_settings(&tx, &chart)?;
        let settings = settings_of(&tx)?;
        tx.commit()?;
        Ok(SetupResult {
            company_name: company,
            chart: applied,
            settings,
        })
    }

    /// Sets the root type of a top-level group. It cannot change once set, and every
    /// typed ledger and group under it must suit it.
    pub fn set_group_root_type(
        &mut self,
        group_id: i64,
        root: RootType,
    ) -> Result<GroupAccount, StoreError> {
        let chart = chart_of(&self.connection)?;
        let group = chart
            .groups
            .iter()
            .find(|g| g.id == group_id)
            .ok_or_else(|| StoreError::Missing("The group does not exist.".into()))?;
        if group.parent_id.is_some() {
            return Err(StoreError::Invalid(
                "Only a top-level group has its own root type; groups inside it take the same one."
                    .into(),
            ));
        }
        if group.root_source == Some(RootSource::Set) && group.root_type != Some(root) {
            return Err(invalid(ot_core::accounts::AccountError::RootChanged));
        }
        // Everything under this group must suit the new root type.
        let mut inside = vec![group_id];
        let mut index = 0;
        while index < inside.len() {
            let current = inside[index];
            inside.extend(
                chart
                    .groups
                    .iter()
                    .filter(|g| g.parent_id == Some(current))
                    .map(|g| g.id),
            );
            index += 1;
        }
        for g in chart.groups.iter().filter(|g| inside.contains(&g.id)) {
            if let Some(kind) = g.account_type {
                check_type_for_root(kind, root).map_err(invalid)?;
            }
        }
        for l in chart
            .ledgers
            .iter()
            .filter(|l| l.group_id.is_some_and(|g| inside.contains(&g)))
        {
            if let Some(kind) = l.account_type {
                check_type_for_root(kind, root).map_err(invalid)?;
            }
        }
        self.connection.execute(
            "INSERT INTO group_accounts (group_id, root_type) VALUES (?, ?)
             ON CONFLICT (group_id) DO UPDATE SET root_type = excluded.root_type",
            params![group_id, root.label()],
        )?;
        resolve_groups(read_groups(&self.connection)?)
            .into_iter()
            .find(|g| g.id == group_id)
            .ok_or(StoreError::Unavailable)
    }

    /// Sets a ledger's account type and GST head. A type, once set, cannot change, and it
    /// must suit the ledger's root type when that is known.
    pub fn set_ledger_account(
        &mut self,
        ledger_id: i64,
        update: LedgerAccountUpdate,
    ) -> Result<LedgerAccount, StoreError> {
        let chart = chart_of(&self.connection)?;
        let ledger = chart
            .ledgers
            .iter()
            .find(|l| l.id == ledger_id)
            .ok_or_else(|| StoreError::Missing("The ledger does not exist.".into()))?;
        check_type_change(ledger.account_type, update.account_type).map_err(invalid)?;
        if let (Some(kind), Some(root)) = (update.account_type, ledger.root_type) {
            check_type_for_root(kind, root).map_err(invalid)?;
        }
        let gst_head = update
            .gst_head
            .map(|h| h.trim().to_owned())
            .filter(|h| !h.is_empty());
        if let Some(head) = &gst_head
            && !["CGST", "SGST", "IGST", "Exempt"].contains(&head.as_str())
        {
            return Err(StoreError::Invalid(
                "Use CGST, SGST, IGST or Exempt as the GST head.".into(),
            ));
        }
        self.connection.execute(
            "INSERT INTO ledger_accounts (ledger_id, account_type, gst_head) VALUES (?, ?, ?)
             ON CONFLICT (ledger_id) DO UPDATE SET account_type = excluded.account_type,
                                                   gst_head = excluded.gst_head",
            params![
                ledger_id,
                update.account_type.map(AccountType::label),
                gst_head
            ],
        )?;
        chart_of(&self.connection)?
            .ledgers
            .into_iter()
            .find(|l| l.id == ledger_id)
            .ok_or(StoreError::Unavailable)
    }

    pub fn update_accounting_settings(
        &mut self,
        update: SettingsUpdate,
    ) -> Result<AccountingSettings, StoreError> {
        let chart = chart_of(&self.connection)?;
        let check_ledger =
            |id: Option<i64>, label: &str, wanted: Option<AccountType>| -> Result<(), StoreError> {
                let Some(id) = id else { return Ok(()) };
                let ledger = chart.ledgers.iter().find(|l| l.id == id).ok_or_else(|| {
                    StoreError::Invalid(format!("Choose an existing ledger for the {label}."))
                })?;
                if let Some(kind) = wanted
                    && ledger.effective_type != Some(kind)
                {
                    return Err(StoreError::Invalid(format!(
                        "The {label} must be a {kind} ledger; {} is not.",
                        ledger.name
                    )));
                }
                Ok(())
            };
        check_ledger(
            update.round_off_ledger_id.flatten(),
            "round-off account",
            None,
        )?;
        check_ledger(
            update.write_off_ledger_id.flatten(),
            "write-off account",
            None,
        )?;
        check_ledger(
            update.discount_ledger_id.flatten(),
            "discount account",
            None,
        )?;
        check_ledger(
            update.receivable_ledger_id.flatten(),
            "default receivable account",
            Some(AccountType::Receivable),
        )?;
        check_ledger(
            update.payable_ledger_id.flatten(),
            "default payable account",
            Some(AccountType::Payable),
        )?;
        let currency = update
            .currency
            .as_deref()
            .map(|c| {
                Currency::new(c)
                    .map(|c| c.code().to_owned())
                    .map_err(invalid)
            })
            .transpose()?;
        if let Some(Some(date)) = &update.fiscal_year_start {
            chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").map_err(|_| {
                StoreError::Invalid("Use YYYY-MM-DD for the fiscal year start.".into())
            })?;
        }
        let gstin = match update.gstin {
            Some(Some(text)) => {
                let text = text.trim().to_uppercase();
                if text.len() != 15 || !text.chars().all(|c| c.is_ascii_alphanumeric()) {
                    return Err(StoreError::Invalid(
                        "A GSTIN has 15 letters and digits.".into(),
                    ));
                }
                Some(Some(text))
            }
            other => other,
        };
        let country = update
            .country
            .map(|c| {
                c.map(|c| required_text(&c, "Country", 100).map(str::to_owned))
                    .transpose()
            })
            .transpose()?;

        let tx = self.connection.transaction()?;
        let set = |column: &str, value: &dyn duckdb::ToSql| -> Result<(), StoreError> {
            tx.execute(
                &format!("UPDATE accounting_settings SET {column} = ? WHERE id = 1"),
                [value],
            )?;
            Ok(())
        };
        if let Some(value) = &country {
            set("country", value)?;
        }
        if let Some(value) = &currency {
            set("currency", value)?;
        }
        if let Some(value) = &update.fiscal_year_start {
            set("fiscal_year_start", &value.as_deref().map(|d| d.to_owned()))?;
        }
        if let Some(value) = &gstin {
            set("gstin", value)?;
        }
        for (column, value) in [
            ("round_off_ledger_id", update.round_off_ledger_id),
            ("write_off_ledger_id", update.write_off_ledger_id),
            ("discount_ledger_id", update.discount_ledger_id),
            ("receivable_ledger_id", update.receivable_ledger_id),
            ("payable_ledger_id", update.payable_ledger_id),
        ] {
            if let Some(value) = value {
                set(column, &value)?;
            }
        }
        if let Some(value) = update.setup_complete {
            set("setup_complete", &value)?;
        }
        let settings = settings_of(&tx)?;
        tx.commit()?;
        Ok(settings)
    }
}

/// A ledger of the given type: the existing one of that name, else a new one in the
/// first group of that type, else in a new group under the right root, as Frappe Books'
/// `ensure_bank_account` and `ensure_discount_account` do. Discounts go under the
/// indirect income group when there is one.
fn ensure_ledger(tx: &Transaction<'_>, name: &str, kind: AccountType) -> Result<i64, StoreError> {
    if let Some((id, _)) = find_ledger(tx, name)? {
        tx.execute(
            "INSERT INTO ledger_accounts (ledger_id, account_type) VALUES (?, ?) ON CONFLICT (ledger_id) DO NOTHING",
            params![id, kind.label()],
        )?;
        return Ok(id);
    }
    let chart = chart_of(tx)?;
    let group = if kind == AccountType::IncomeAccount {
        chart
            .groups
            .iter()
            .find(|g| {
                ["indirect income", "indirect incomes"].contains(&g.name.to_lowercase().as_str())
            })
            .or_else(|| {
                chart
                    .groups
                    .iter()
                    .find(|g| g.root_type == Some(RootType::Income))
            })
    } else {
        chart.groups.iter().find(|g| g.account_type == Some(kind))
    }
    .map(|g| g.id);
    let group_id =
        match group {
            Some(id) => id,
            None => {
                let (group_name, root) = match kind {
                    AccountType::Bank => ("Bank Accounts", RootType::Asset),
                    _ => ("Indirect Income", RootType::Income),
                };
                let id: i64 = tx.query_row(
                    "INSERT INTO account_groups (name, name_key) VALUES (?, ?) RETURNING id",
                    params![group_name, group_name.to_lowercase()],
                    |row| row.get(0),
                )?;
                tx.execute(
                "INSERT INTO group_accounts (group_id, root_type, account_type) VALUES (?, ?, ?)",
                params![id, root.label(), (kind == AccountType::Bank).then_some(kind.label())],
            )?;
                id
            }
        };
    let ledger_kind = if kind == AccountType::Bank {
        "bank"
    } else {
        "general"
    };
    let id: i64 = tx.query_row(
        "INSERT INTO ledgers (name, name_key, kind) VALUES (?, ?, ?) RETURNING id",
        params![name, name.to_lowercase(), ledger_kind],
        |row| row.get(0),
    )?;
    tx.execute(
        "INSERT INTO ledger_groups VALUES (?, ?)",
        params![id, group_id],
    )?;
    tx.execute(
        "INSERT INTO ledger_accounts (ledger_id, account_type) VALUES (?, ?)",
        params![id, kind.label()],
    )?;
    Ok(id)
}

/// Applies a chart template inside a transaction; see [`Store::apply_chart`].
fn apply_chart_in(tx: &Transaction<'_>, chart: &str) -> Result<ChartApplied, StoreError> {
    let accounts = load_chart(chart).map_err(invalid)?;
    let (mut groups_created, mut ledgers_created, mut ledgers_placed) = (0, 0, 0);
    let mut group_ids: HashMap<String, i64> = HashMap::new();

    for account in &accounts {
        let parent_id = account
            .parent
            .as_ref()
            .and_then(|p| group_ids.get(p).copied());
        if account.is_group {
            let id = match find_group(tx, &account.name)? {
                Some(id) => id,
                None => {
                    groups_created += 1;
                    tx.query_row(
                        "INSERT INTO account_groups (name, name_key, parent_id) VALUES (?, ?, ?) RETURNING id",
                        params![account.name, account.name.to_lowercase(), parent_id],
                        |row| row.get(0),
                    )?
                }
            };
            // Classify the group unless someone already did.
            tx.execute(
                "INSERT INTO group_accounts (group_id, root_type, account_type) VALUES (?, ?, ?)
                 ON CONFLICT (group_id) DO NOTHING",
                params![
                    id,
                    account.root_type.label(),
                    account.account_type.map(AccountType::label)
                ],
            )?;
            group_ids.insert(account.name.clone(), id);
            continue;
        }
        let Some(group_id) = parent_id else { continue };
        let ledger_id = match find_ledger(tx, &account.name)? {
            Some((id, existing_group)) => {
                if existing_group.is_none() {
                    tx.execute(
                        "INSERT INTO ledger_groups VALUES (?, ?)",
                        params![id, group_id],
                    )?;
                    ledgers_placed += 1;
                }
                id
            }
            None => {
                ledgers_created += 1;
                let kind = match account.account_type {
                    Some(AccountType::Cash) => "cash",
                    Some(AccountType::Bank) => "bank",
                    _ => "general",
                };
                let id: i64 = tx.query_row(
                    "INSERT INTO ledgers (name, name_key, kind) VALUES (?, ?, ?) RETURNING id",
                    params![account.name, account.name.to_lowercase(), kind],
                    |row| row.get(0),
                )?;
                tx.execute(
                    "INSERT INTO ledger_groups VALUES (?, ?)",
                    params![id, group_id],
                )?;
                id
            }
        };
        if let Some(kind) = account.account_type {
            tx.execute(
                "INSERT INTO ledger_accounts (ledger_id, account_type) VALUES (?, ?)
                 ON CONFLICT (ledger_id) DO NOTHING",
                params![ledger_id, kind.label()],
            )?;
        }
    }

    // Ungrouped cash and bank ledgers go to the chart's first cash or bank group.
    for (kind, account_type) in [("cash", AccountType::Cash), ("bank", AccountType::Bank)] {
        let target = accounts
            .iter()
            .find(|a| a.is_group && a.account_type == Some(account_type))
            .and_then(|a| group_ids.get(&a.name).copied());
        if let Some(group_id) = target {
            ledgers_placed += tx.execute(
                "INSERT INTO ledger_groups (ledger_id, group_id)
                 SELECT id, ? FROM ledgers WHERE kind = ?
                   AND id NOT IN (SELECT ledger_id FROM ledger_groups)",
                params![group_id, kind],
            )?;
        }
    }

    fill_default_settings(tx, chart)?;
    let settings = settings_of(tx)?;
    Ok(ChartApplied {
        chart: chart.to_owned(),
        groups_created,
        ledgers_created,
        ledgers_placed,
        settings,
    })
}

/// Fills empty default-account settings from the book's ledgers, as Frappe Books' setup
/// does: the first Round Off, Receivable and Payable ledgers (or the ones the chart
/// names), and the Discounts and Write Off ledgers.
fn fill_default_settings(tx: &Transaction<'_>, chart: &str) -> Result<(), StoreError> {
    let accounts = chart_of(tx)?;
    let named = ot_core::accounts::charts()
        .into_iter()
        .find(|c| c.name == chart);
    let pick = |kind: AccountType, name: Option<&str>| -> Option<i64> {
        let of_kind = accounts
            .ledgers
            .iter()
            .filter(|l| l.effective_type == Some(kind));
        name.and_then(|n| of_kind.clone().find(|l| l.name == n))
            .or_else(|| of_kind.clone().next())
            .map(|l| l.id)
    };
    let by_name = |name: &str| {
        accounts
            .ledgers
            .iter()
            .find(|l| l.name.eq_ignore_ascii_case(name))
            .map(|l| l.id)
    };
    let defaults = [
        ("round_off_ledger_id", pick(AccountType::RoundOff, None)),
        (
            "receivable_ledger_id",
            pick(
                AccountType::Receivable,
                named.as_ref().and_then(|c| c.receivable.as_deref()),
            ),
        ),
        (
            "payable_ledger_id",
            pick(
                AccountType::Payable,
                named.as_ref().and_then(|c| c.payable.as_deref()),
            ),
        ),
        ("discount_ledger_id", by_name("Discounts")),
        ("write_off_ledger_id", by_name("Write Off")),
    ];
    for (column, value) in defaults {
        if let Some(id) = value {
            tx.execute(
                &format!(
                    "UPDATE accounting_settings SET {column} = ? WHERE id = 1 AND {column} IS NULL"
                ),
                [id],
            )?;
        }
    }
    tx.execute(
        "UPDATE accounting_settings SET chart = ? WHERE id = 1 AND chart IS NULL",
        [chart],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use ot_core::accounts::TALLY_CHART;
    use serde_json::json;

    use super::*;

    fn book() -> Store {
        Store::open_in_memory().unwrap()
    }

    fn decode<T: serde::de::DeserializeOwned>(value: serde_json::Value) -> T {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn a_new_book_has_unclassified_cash_and_bank() {
        let store = book();
        let chart = store.chart_of_accounts().unwrap();
        assert!(chart.groups.is_empty());
        assert_eq!(chart.unclassified_ledgers, 2);
        let cash = chart.ledgers.iter().find(|l| l.name == "Cash").unwrap();
        assert_eq!(
            (cash.account_type, cash.effective_type),
            (None, Some(AccountType::Cash))
        );
        let settings = store.accounting_settings().unwrap();
        assert_eq!(
            (settings.currency.as_str(), settings.setup_complete),
            ("INR", false)
        );
    }

    #[test]
    fn the_tally_chart_classifies_the_book_and_fills_settings() {
        let mut store = book();
        let applied = store.apply_chart(TALLY_CHART).unwrap();
        assert_eq!(applied.groups_created, 28);
        // Cash already existed: it is placed, not created; the other three are new.
        assert_eq!(applied.ledgers_created, 3);
        let chart = store.chart_of_accounts().unwrap();
        assert_eq!(chart.unclassified_ledgers, 0, "{:?}", chart.ledgers);
        let ledger = |name: &str| {
            chart
                .ledgers
                .iter()
                .find(|l| l.name == name)
                .unwrap()
                .clone()
        };
        let group = |id: i64| chart.groups.iter().find(|g| g.id == id).unwrap().clone();
        assert_eq!(group(ledger("Cash").group_id.unwrap()).name, "Cash-in-Hand");
        assert_eq!(
            group(ledger("Bank").group_id.unwrap()).name,
            "Bank Accounts"
        );
        assert_eq!(ledger("Bank").root_type, Some(RootType::Asset));
        let settings = &applied.settings;
        assert_eq!(settings.round_off_ledger_id, Some(ledger("Rounded Off").id));
        assert_eq!(settings.discount_ledger_id, Some(ledger("Discounts").id));
        assert_eq!(settings.chart.as_deref(), Some(TALLY_CHART));
        // Applying it again changes nothing.
        let again = store.apply_chart(TALLY_CHART).unwrap();
        assert_eq!(
            (
                again.groups_created,
                again.ledgers_created,
                again.ledgers_placed
            ),
            (0, 0, 0)
        );
    }

    #[test]
    fn the_india_chart_sets_receivable_and_payable_defaults() {
        let mut store = book();
        let applied = store.apply_chart("India - Chart of Accounts").unwrap();
        let chart = store.chart_of_accounts().unwrap();
        let name = |id: Option<i64>| {
            chart
                .ledgers
                .iter()
                .find(|l| Some(l.id) == id)
                .map(|l| l.name.clone())
        };
        assert_eq!(
            name(applied.settings.receivable_ledger_id).as_deref(),
            Some("Debtors")
        );
        assert_eq!(
            name(applied.settings.payable_ledger_id).as_deref(),
            Some("Creditors")
        );
        assert_eq!(
            name(applied.settings.round_off_ledger_id).as_deref(),
            Some("Rounded Off")
        );
        let gst = chart.ledgers.iter().find(|l| l.name == "CGST").unwrap();
        assert_eq!(
            (gst.root_type, gst.effective_type),
            (Some(RootType::Liability), Some(AccountType::Tax))
        );
    }

    #[test]
    fn tally_named_groups_are_classified_without_being_set() {
        let store = book();
        let assets = store
            .add_group(decode(json!({"name": "Current Assets"})))
            .unwrap();
        let debtors = store
            .add_group(decode(
                json!({"name": "Sundry Debtors", "parent_id": assets.id()}),
            ))
            .unwrap();
        let customers = store
            .add_group(decode(
                json!({"name": "Mumbai customers", "parent_id": debtors.id()}),
            ))
            .unwrap();
        let other = store
            .add_group(decode(json!({"name": "Odds and ends"})))
            .unwrap();
        let chart = store.chart_of_accounts().unwrap();
        let find = |id: i64| chart.groups.iter().find(|g| g.id == id).unwrap();
        assert_eq!(
            (
                find(customers.id()).root_type,
                find(customers.id()).root_source
            ),
            (Some(RootType::Asset), Some(RootSource::Inherited))
        );
        assert_eq!(find(debtors.id()).root_source, Some(RootSource::TallyName));
        assert_eq!(find(other.id()).root_type, None);
    }

    #[test]
    fn root_types_are_set_once_and_must_suit_the_accounts_inside() {
        let mut store = book();
        let group = store
            .add_group(decode(json!({"name": "Odds and ends"})))
            .unwrap();
        let child = store
            .add_group(decode(json!({"name": "Inner", "parent_id": group.id()})))
            .unwrap();
        assert!(matches!(
            store.set_group_root_type(child.id(), RootType::Asset),
            Err(StoreError::Invalid(_))
        ));
        let ledger: crate::Ledger = store
            .add_ledger(decode(
                json!({"name": "Shop rent", "kind": "general", "group_id": child.id()}),
            ))
            .unwrap();
        let ledger_id = serde_json::to_value(&ledger).unwrap()["id"]
            .as_i64()
            .unwrap();
        store
            .set_ledger_account(
                ledger_id,
                LedgerAccountUpdate {
                    account_type: Some(AccountType::ExpenseAccount),
                    gst_head: None,
                },
            )
            .unwrap();
        // An Expense Account cannot sit under an Asset root.
        assert!(matches!(
            store.set_group_root_type(group.id(), RootType::Asset),
            Err(StoreError::Invalid(_))
        ));
        store
            .set_group_root_type(group.id(), RootType::Expense)
            .unwrap();
        assert!(
            store
                .set_group_root_type(group.id(), RootType::Expense)
                .is_ok()
        );
        assert!(matches!(
            store.set_group_root_type(group.id(), RootType::Income),
            Err(StoreError::Invalid(_))
        ));
        // The ledger's type is set once.
        let change = store.set_ledger_account(
            ledger_id,
            LedgerAccountUpdate {
                account_type: Some(AccountType::Tax),
                gst_head: None,
            },
        );
        assert!(matches!(change, Err(StoreError::Invalid(_))));
        let head = store.set_ledger_account(
            ledger_id,
            LedgerAccountUpdate {
                account_type: Some(AccountType::ExpenseAccount),
                gst_head: Some("GST".into()),
            },
        );
        assert!(matches!(head, Err(StoreError::Invalid(_))));
    }

    #[test]
    fn settings_check_their_ledgers_and_formats() {
        let mut store = book();
        store.apply_chart("India - Chart of Accounts").unwrap();
        let chart = store.chart_of_accounts().unwrap();
        let id = |name: &str| chart.ledgers.iter().find(|l| l.name == name).unwrap().id;
        let wrong = store
            .update_accounting_settings(decode(json!({"receivable_ledger_id": id("Creditors")})));
        assert!(matches!(wrong, Err(StoreError::Invalid(_))));
        assert!(
            store
                .update_accounting_settings(decode(json!({"currency": "rupee"})))
                .is_err()
        );
        assert!(
            store
                .update_accounting_settings(decode(json!({"gstin": "123"})))
                .is_err()
        );
        assert!(
            store
                .update_accounting_settings(decode(json!({"fiscal_year_start": "April"})))
                .is_err()
        );
        let saved = store
            .update_accounting_settings(decode(json!({
                "country": "India", "currency": "inr", "fiscal_year_start": "2025-04-01",
                "gstin": "27aapfu0939f1zv", "setup_complete": true, "discount_ledger_id": null
            })))
            .unwrap();
        assert_eq!(saved.currency, "INR");
        assert_eq!(saved.gstin.as_deref(), Some("27AAPFU0939F1ZV"));
        assert_eq!(saved.fiscal_year_start.as_deref(), Some("2025-04-01"));
        assert_eq!(saved.discount_ledger_id, None);
        assert!(saved.setup_complete);
        assert!(serde_json::from_value::<SettingsUpdate>(json!({"unknown": 1})).is_err());
    }

    fn day(text: &str) -> chrono::NaiveDate {
        chrono::NaiveDate::parse_from_str(text, "%Y-%m-%d").unwrap()
    }

    // Ported from Frappe Books' run_setup: chart, bank, discounts, defaults, once only.
    #[test]
    fn setup_creates_the_company_once() {
        let mut store = book();
        let result = store
            .setup_company(
                decode(json!({"company_name": "Shree Traders", "country": "India",
                              "bank_name": "HDFC Bank", "gstin": "27aapfu0939f1zv"})),
                day("2026-11-20"),
            )
            .unwrap();
        let settings = &result.settings;
        assert!(settings.setup_complete);
        assert_eq!(settings.currency, "INR");
        assert_eq!(settings.chart.as_deref(), Some(TALLY_CHART));
        assert_eq!(
            (
                settings.fiscal_year_start.as_deref(),
                settings.fiscal_year_end.as_deref()
            ),
            (Some("2026-04-01"), Some("2027-03-31"))
        );
        assert_eq!(settings.gstin.as_deref(), Some("27AAPFU0939F1ZV"));
        let chart = store.chart_of_accounts().unwrap();
        let bank = chart
            .ledgers
            .iter()
            .find(|l| l.name == "HDFC Bank")
            .unwrap();
        assert_eq!(
            (bank.kind.as_str(), bank.effective_type),
            ("bank", Some(AccountType::Bank))
        );
        assert_eq!(settings.bank_ledger_id, Some(bank.id));
        assert_eq!(
            chart
                .groups
                .iter()
                .find(|g| Some(g.id) == bank.group_id)
                .unwrap()
                .name,
            "Bank Accounts"
        );
        assert!(settings.discount_ledger_id.is_some() && settings.round_off_ledger_id.is_some());
        assert_eq!(
            serde_json::to_value(store.settings().unwrap()).unwrap()["company_name"],
            "Shree Traders"
        );
        let again = store.setup_company(
            decode(json!({"company_name": "X", "country": "India", "bank_name": "Y"})),
            day("2026-11-20"),
        );
        assert!(matches!(again, Err(StoreError::Conflict(_))));
    }

    #[test]
    fn setup_with_a_frappe_chart_adds_discounts_under_indirect_income() {
        let mut store = book();
        let result = store
            .setup_company(
                decode(json!({"company_name": "Acme", "country": "India", "bank_name": "Axis",
                              "chart": "India - Chart of Accounts", "fiscal_year_start": "2026-01-01"})),
                day("2026-11-20"),
            )
            .unwrap();
        assert_eq!(
            result.settings.fiscal_year_end.as_deref(),
            Some("2026-12-31")
        );
        let chart = store.chart_of_accounts().unwrap();
        let discounts = chart
            .ledgers
            .iter()
            .find(|l| l.name == "Discounts")
            .unwrap();
        let group = chart
            .groups
            .iter()
            .find(|g| Some(g.id) == discounts.group_id)
            .unwrap();
        assert_eq!(
            (group.name.as_str(), discounts.root_type),
            ("Indirect Income", Some(RootType::Income))
        );
        assert_eq!(result.settings.discount_ledger_id, Some(discounts.id));
    }

    #[test]
    fn setup_checks_its_input_and_changes_nothing_when_it_fails() {
        let mut store = book();
        for body in [
            json!({"company_name": " ", "country": "India", "bank_name": "B"}),
            json!({"company_name": "A", "country": "Atlantis", "bank_name": "B"}),
            json!({"company_name": "A", "country": "India", "bank_name": "B", "currency": "rupee"}),
            json!({"company_name": "A", "country": "India", "bank_name": "B", "chart": "Nowhere"}),
            json!({"company_name": "A", "country": "India", "bank_name": "B",
                   "fiscal_year_start": "2026-04-01", "fiscal_year_end": "2027-04-01"}),
            json!({"company_name": "A", "country": "India", "bank_name": "B", "gstin": "12"}),
        ] {
            let result = store.setup_company(decode(body.clone()), day("2026-11-20"));
            assert!(matches!(result, Err(StoreError::Invalid(_))), "{body}");
        }
        assert!(store.chart_of_accounts().unwrap().groups.is_empty());
        assert!(!store.accounting_settings().unwrap().setup_complete);
    }

    #[test]
    fn trial_balance_is_unchanged_by_applying_a_chart() {
        let mut store = book();
        let expense: crate::Ledger = store
            .add_ledger(decode(json!({"name": "Tea", "kind": "general"})))
            .unwrap();
        let expense = serde_json::to_value(&expense).unwrap()["id"]
            .as_i64()
            .unwrap();
        store
            .add_voucher(decode(json!({"kind": "payment", "request_id": "29daeb83-d7be-4576-aca0-000000000001",
                "date": "2025-09-03", "amount": "250.00", "cash_ledger_id": 1, "other_ledger_id": expense})))
            .unwrap();
        let period = json!({"from": "2025-09-01", "to": "2025-09-30"});
        let before =
            serde_json::to_value(store.trial_balance(decode(period.clone())).unwrap()).unwrap();
        store.apply_chart(TALLY_CHART).unwrap();
        let after = serde_json::to_value(store.trial_balance(decode(period)).unwrap()).unwrap();
        for key in ["debit_paise", "credit_paise"] {
            assert_eq!(before[key], after[key], "{key}");
        }
    }
}
