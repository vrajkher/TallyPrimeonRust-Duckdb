// Derived from Frappe Books (AGPL-3.0)
// frappe_books/frappe_books/doctype/books_tax/books_tax.py, books_tax_detail and
// frappe_books/regional.py (ensure_regional_records, GST_RATES, GST_HEADS)
// @953e9b4, modified: rates are bounded to 0–100% with four decimals, and a tax that
// items use cannot be deleted.

//! Tax templates (roadmap step 1.7), and India's GST taxes.

use std::{collections::HashSet, str::FromStr};

use duckdb::{Connection, OptionalExt, Transaction, params};
use ot_core::{accounts::AccountType, money::Decimal};
use serde::{Deserialize, Serialize};

use crate::{
    chart::{chart_of, ensure_ledger},
    store::{Store, StoreError, required_text},
};

/// India's GST rates, as Frappe Books seeds them.
pub const GST_RATES: &[&str] = &["28", "18", "12", "6", "5", "3", "0.25", "0"];
/// The GST ledgers, named after the GST head they hold.
pub const GST_HEADS: &[&str] = &["CGST", "SGST", "IGST", "Exempt"];

/// One row of a tax: `rate` percent posted to a ledger.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaxDetailInput {
    pub ledger_id: i64,
    #[serde(default)]
    pub payment_ledger_id: Option<i64>,
    /// Percent as a decimal number, e.g. "9" or "0.125".
    pub rate: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct TaxInput {
    pub name: String,
    pub details: Vec<TaxDetailInput>,
    #[serde(default)]
    pub revision: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TaxDetail {
    pub ledger_id: i64,
    pub ledger_name: String,
    pub payment_ledger_id: Option<i64>,
    pub payment_ledger_name: Option<String>,
    pub rate: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Tax {
    pub id: i64,
    pub name: String,
    pub details: Vec<TaxDetail>,
    /// The sum of the rows' rates, e.g. "18" for CGST 9 + SGST 9.
    pub total_rate: String,
    /// How many items and item groups name this tax.
    pub used_by: i64,
    pub revision: i32,
}

fn invalid(text: impl Into<String>) -> StoreError {
    StoreError::Invalid(text.into())
}

fn parse_rate(text: &str) -> Result<Decimal, StoreError> {
    Decimal::from_str(text.trim())
        .ok()
        .filter(|r| *r >= Decimal::ZERO && *r <= Decimal::from(100) && r.normalize().scale() <= 4)
        .ok_or_else(|| {
            invalid(format!(
                "{} is not a tax rate. Enter a percentage from 0 to 100, with at most four decimals.",
                text.trim()
            ))
        })
}

/// Checks a tax's rows: at least one, ledgers that exist (OpenTally's ledgers are
/// always leaf accounts, as Frappe's validate_leaf_accounts requires) and valid rates.
fn clean_details(
    conn: &Connection,
    details: &[TaxDetailInput],
) -> Result<Vec<(i64, Option<i64>, Decimal)>, StoreError> {
    if details.is_empty() {
        return Err(invalid("Add at least one tax row."));
    }
    let ledgers: HashSet<i64> = chart_of(conn)?.ledgers.iter().map(|l| l.id).collect();
    details
        .iter()
        .map(|row| {
            for id in std::iter::once(row.ledger_id).chain(row.payment_ledger_id) {
                if !ledgers.contains(&id) {
                    return Err(invalid(
                        "Each tax row must post to an existing ledger, not a group.",
                    ));
                }
            }
            Ok((row.ledger_id, row.payment_ledger_id, parse_rate(&row.rate)?))
        })
        .collect()
}

fn name_taken(conn: &Connection, name: &str, except: Option<i64>) -> Result<bool, StoreError> {
    Ok(conn.query_row(
        "SELECT COUNT(*) > 0 FROM taxes WHERE name_key = ? AND id <> ?",
        params![name.trim().to_lowercase(), except.unwrap_or(-1)],
        |row| row.get(0),
    )?)
}

fn save_details(
    conn: &Connection,
    id: i64,
    rows: &[(i64, Option<i64>, Decimal)],
) -> Result<(), StoreError> {
    conn.execute("DELETE FROM tax_details WHERE tax_id = ?", params![id])?;
    for (idx, (ledger, payment, rate)) in rows.iter().enumerate() {
        conn.execute(
            "INSERT INTO tax_details (tax_id, idx, ledger_id, payment_ledger_id, rate)
             VALUES (?, ?, ?, ?, CAST(? AS DECIMAL(9, 4)))",
            params![id, idx as i64 + 1, ledger, payment, rate.to_string()],
        )?;
    }
    Ok(())
}

fn read_taxes(conn: &Connection, only: Option<i64>) -> Result<Vec<Tax>, StoreError> {
    let mut statement = conn.prepare(
        "SELECT t.id, t.name, t.revision,
                (SELECT COUNT(*) FROM items i WHERE i.tax_id = t.id)
                + (SELECT COUNT(*) FROM item_groups g WHERE g.tax_id = t.id)
         FROM taxes t WHERE (CAST(? AS BIGINT) IS NULL OR t.id = ?) ORDER BY t.name_key",
    )?;
    let mut taxes = statement
        .query_map(params![only, only], |row| {
            Ok(Tax {
                id: row.get(0)?,
                name: row.get(1)?,
                revision: row.get(2)?,
                used_by: row.get(3)?,
                details: Vec::new(),
                total_rate: String::new(),
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut details = conn.prepare(
        "SELECT d.ledger_id, l.name, d.payment_ledger_id, p.name, CAST(d.rate AS VARCHAR)
         FROM tax_details d
         LEFT JOIN ledgers l ON l.id = d.ledger_id
         LEFT JOIN ledgers p ON p.id = d.payment_ledger_id
         WHERE d.tax_id = ? ORDER BY d.idx",
    )?;
    for tax in &mut taxes {
        let mut total = Decimal::ZERO;
        tax.details = details
            .query_map(params![tax.id], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })?
            .map(|row| {
                let (ledger_id, ledger_name, payment_ledger_id, payment_ledger_name, rate) = row?;
                let rate = Decimal::from_str(&rate).unwrap_or_default().normalize();
                total += rate;
                Ok(TaxDetail {
                    ledger_id,
                    ledger_name: ledger_name.unwrap_or_default(),
                    payment_ledger_id,
                    payment_ledger_name,
                    rate: rate.to_string(),
                })
            })
            .collect::<Result<Vec<_>, duckdb::Error>>()?;
        tax.total_rate = total.normalize().to_string();
    }
    Ok(taxes)
}

fn get(conn: &Connection, id: i64) -> Result<Tax, StoreError> {
    read_taxes(conn, Some(id))?
        .pop()
        .ok_or_else(|| StoreError::Missing("Tax not found.".into()))
}

/// Checks that a tax exists, for items and item groups that name one.
pub(crate) fn check_tax(conn: &Connection, id: Option<i64>) -> Result<Option<i64>, StoreError> {
    if let Some(id) = id {
        let exists: bool = conn.query_row(
            "SELECT COUNT(*) > 0 FROM taxes WHERE id = ?",
            params![id],
            |row| row.get(0),
        )?;
        if !exists {
            return Err(invalid("Choose an existing tax."));
        }
    }
    Ok(id)
}

/// India's GST: the CGST, SGST, IGST and Exempt ledgers under Duties and Taxes, and
/// the taxes GST-<rate> (CGST and SGST at half the rate each), IGST-<rate>,
/// Exempt-GST-0 and Exempt-IGST-0. Existing ledgers and taxes are kept.
pub(crate) fn seed_gst(tx: &Transaction<'_>) -> Result<(), StoreError> {
    let mut heads = Vec::with_capacity(GST_HEADS.len());
    for head in GST_HEADS {
        let id = ensure_ledger(tx, head, AccountType::Tax)?;
        tx.execute(
            "UPDATE ledger_accounts SET gst_head = ? WHERE ledger_id = ?",
            params![head, id],
        )?;
        heads.push(id);
    }
    let [cgst, sgst, igst, exempt] = heads[..] else {
        unreachable!("four GST heads");
    };
    let mut taxes: Vec<(String, Vec<(i64, Decimal)>)> = Vec::new();
    for rate in GST_RATES {
        let rate = Decimal::from_str(rate).expect("GST rates are numbers");
        let half = (rate / Decimal::from(2)).normalize();
        taxes.push((format!("GST-{rate}"), vec![(cgst, half), (sgst, half)]));
        taxes.push((format!("IGST-{rate}"), vec![(igst, rate)]));
    }
    taxes.push(("Exempt-GST-0".into(), vec![(exempt, Decimal::ZERO)]));
    taxes.push(("Exempt-IGST-0".into(), vec![(exempt, Decimal::ZERO)]));
    for (name, rows) in taxes {
        if name_taken(tx, &name, None)? {
            continue;
        }
        let id: i64 = tx.query_row(
            "INSERT INTO taxes (name, name_key) VALUES (?, ?) RETURNING id",
            params![name, name.to_lowercase()],
            |row| row.get(0),
        )?;
        let rows: Vec<_> = rows.into_iter().map(|(l, r)| (l, None, r)).collect();
        save_details(tx, id, &rows)?;
    }
    Ok(())
}

impl Store {
    pub fn taxes(&self) -> Result<Vec<Tax>, StoreError> {
        read_taxes(&self.connection, None)
    }

    pub fn tax(&self, id: i64) -> Result<Tax, StoreError> {
        get(&self.connection, id)
    }

    pub fn add_tax(&mut self, input: TaxInput) -> Result<Tax, StoreError> {
        let name = required_text(&input.name, "the tax name", 140)?.to_owned();
        let tx = self.connection.transaction()?;
        let rows = clean_details(&tx, &input.details)?;
        if name_taken(&tx, &name, None)? {
            return Err(StoreError::Conflict(format!(
                "A tax named {name} already exists."
            )));
        }
        let id: i64 = tx.query_row(
            "INSERT INTO taxes (name, name_key) VALUES (?, ?) RETURNING id",
            params![name, name.to_lowercase()],
            |row| row.get(0),
        )?;
        save_details(&tx, id, &rows)?;
        let saved = get(&tx, id)?;
        tx.commit()?;
        Ok(saved)
    }

    /// Replaces a tax's name and rows. `input.revision` must be its current revision.
    pub fn update_tax(&mut self, id: i64, input: TaxInput) -> Result<Tax, StoreError> {
        let name = required_text(&input.name, "the tax name", 140)?.to_owned();
        let tx = self.connection.transaction()?;
        let current = get(&tx, id)?;
        match input.revision {
            Some(revision) if revision == current.revision => {}
            Some(_) => {
                return Err(StoreError::Conflict(format!(
                    "{} was changed by someone else. Reload it and try again.",
                    current.name
                )));
            }
            None => return Err(invalid("Send the revision you are changing.")),
        }
        let rows = clean_details(&tx, &input.details)?;
        if name_taken(&tx, &name, Some(id))? {
            return Err(StoreError::Conflict(format!(
                "A tax named {name} already exists."
            )));
        }
        tx.execute(
            "UPDATE taxes SET name = ?, name_key = ?, revision = revision + 1 WHERE id = ?",
            params![name, name.to_lowercase(), id],
        )?;
        save_details(&tx, id, &rows)?;
        let saved = get(&tx, id)?;
        tx.commit()?;
        Ok(saved)
    }

    /// Deletes a tax that no item or item group names.
    pub fn delete_tax(&mut self, id: i64) -> Result<(), StoreError> {
        let tx = self.connection.transaction()?;
        let tax = get(&tx, id)?;
        if tax.used_by > 0 {
            return Err(invalid(format!(
                "{} is used by {} item(s) or group(s), so it cannot be deleted.",
                tax.name, tax.used_by
            )));
        }
        tx.execute("DELETE FROM tax_details WHERE tax_id = ?", params![id])?;
        tx.execute("DELETE FROM taxes WHERE id = ?", params![id])?;
        tx.commit()?;
        Ok(())
    }

    /// Adds India's GST ledgers and taxes to a book whose company was set up before
    /// they were seeded, or whose chart lacks them. Existing ones are kept.
    pub fn add_gst_taxes(&mut self) -> Result<Vec<Tax>, StoreError> {
        let tx = self.connection.transaction()?;
        seed_gst(&tx)?;
        tx.commit()?;
        self.taxes()
    }
}

/// A tax by name, for tests and the command line.
pub fn find_tax(store: &Store, name: &str) -> Result<Option<Tax>, StoreError> {
    let id: Option<i64> = store
        .connection
        .query_row(
            "SELECT id FROM taxes WHERE name_key = ?",
            params![name.trim().to_lowercase()],
            |row| row.get(0),
        )
        .optional()?;
    id.map(|id| store.tax(id)).transpose()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chart::CompanySetup;

    fn store(country: &str) -> Store {
        let mut store = Store::open_in_memory().unwrap();
        let setup: CompanySetup = serde_json::from_value(serde_json::json!({
            "company_name": "Shree Traders", "country": country, "bank_name": "HDFC Bank"
        }))
        .unwrap();
        store
            .setup_company(setup, chrono::NaiveDate::from_ymd_opt(2026, 6, 1).unwrap())
            .unwrap();
        store
    }

    fn ledger(store: &Store, name: &str) -> i64 {
        store
            .chart_of_accounts()
            .unwrap()
            .ledgers
            .into_iter()
            .find(|l| l.name == name)
            .unwrap()
            .id
    }

    // Ported from ensure_regional_records: an Indian company gets the GST taxes.
    #[test]
    fn an_indian_company_gets_gst_ledgers_and_taxes() {
        let mut store = store("India");
        let taxes = store.taxes().unwrap();
        assert_eq!(taxes.len(), GST_RATES.len() * 2 + 2);
        let gst18 = find_tax(&store, "GST-18").unwrap().unwrap();
        let rows: Vec<(&str, &str)> = gst18
            .details
            .iter()
            .map(|d| (d.ledger_name.as_str(), d.rate.as_str()))
            .collect();
        assert_eq!(rows, [("CGST", "9"), ("SGST", "9")]);
        assert_eq!(gst18.total_rate, "18");
        let quarter = find_tax(&store, "GST-0.25").unwrap().unwrap();
        assert_eq!(quarter.details[0].rate, "0.125");
        assert_eq!(
            find_tax(&store, "IGST-28").unwrap().unwrap().total_rate,
            "28"
        );
        assert_eq!(
            find_tax(&store, "Exempt-IGST-0").unwrap().unwrap().details[0].ledger_name,
            "Exempt"
        );
        // The heads are tax ledgers with their GST head, in the Tally Duties & Taxes group.
        let chart = store.chart_of_accounts().unwrap();
        let cgst = chart.ledgers.iter().find(|l| l.name == "CGST").unwrap();
        assert_eq!(cgst.gst_head.as_deref(), Some("CGST"));
        assert_eq!(cgst.effective_type, Some(AccountType::Tax));
        let group = chart
            .groups
            .iter()
            .find(|g| Some(g.id) == cgst.group_id)
            .unwrap();
        assert_eq!(group.name, "Duties & Taxes");
        // Seeding again keeps what is there.
        store.add_gst_taxes().unwrap();
        assert_eq!(store.taxes().unwrap().len(), taxes.len());
    }

    #[test]
    fn other_countries_start_without_gst() {
        let store = store("United States");
        assert!(store.taxes().unwrap().is_empty());
    }

    // Ported in spirit from test_tax_rows_take_ledger_accounts_only, with OpenTally's
    // own rate and row checks.
    #[test]
    fn tax_rows_need_ledgers_and_valid_rates() {
        let mut store = store("United States");
        assert!(matches!(
            store.add_tax(TaxInput {
                name: "Sales Tax".into(),
                details: vec![],
                revision: None
            }),
            Err(StoreError::Invalid(_))
        ));
        let row = |ledger_id, rate: &str| TaxDetailInput {
            ledger_id,
            payment_ledger_id: None,
            rate: rate.into(),
        };
        let cash = ledger(&store, "Cash");
        for bad in [
            row(999_999, "8"),
            row(cash, "-1"),
            row(cash, "101"),
            row(cash, "1.23456"),
            row(cash, "eight"),
        ] {
            assert!(
                matches!(
                    store.add_tax(TaxInput {
                        name: "Sales Tax".into(),
                        details: vec![bad.clone()],
                        revision: None
                    }),
                    Err(StoreError::Invalid(_))
                ),
                "{bad:?}"
            );
        }
        let mut payment = row(cash, "8.875");
        payment.payment_ledger_id = Some(999_999);
        assert!(matches!(
            store.add_tax(TaxInput {
                name: "Sales Tax".into(),
                details: vec![payment],
                revision: None
            }),
            Err(StoreError::Invalid(_))
        ));
        let tax = store
            .add_tax(TaxInput {
                name: "Sales Tax".into(),
                details: vec![row(cash, "8.875"), row(cash, "0.5")],
                revision: None,
            })
            .unwrap();
        assert_eq!(tax.total_rate, "9.375");
        assert!(matches!(
            store.add_tax(TaxInput {
                name: "SALES TAX".into(),
                details: vec![row(cash, "1")],
                revision: None
            }),
            Err(StoreError::Conflict(_))
        ));
    }

    #[test]
    fn changes_need_the_revision_and_used_taxes_stay() {
        let mut store = store("India");
        let gst5 = find_tax(&store, "GST-5").unwrap().unwrap();
        let mut input = TaxInput {
            name: "GST-5 (food)".into(),
            details: gst5
                .details
                .iter()
                .map(|d| TaxDetailInput {
                    ledger_id: d.ledger_id,
                    payment_ledger_id: None,
                    rate: d.rate.clone(),
                })
                .collect(),
            revision: None,
        };
        assert!(matches!(
            store.update_tax(gst5.id, input.clone()),
            Err(StoreError::Invalid(_))
        ));
        input.revision = Some(gst5.revision);
        let renamed = store.update_tax(gst5.id, input.clone()).unwrap();
        assert_eq!(
            (renamed.name.as_str(), renamed.revision),
            ("GST-5 (food)", 2)
        );
        assert!(matches!(
            store.update_tax(gst5.id, input),
            Err(StoreError::Conflict(_))
        ));

        let mut rice = crate::items::ItemInput {
            name: "Basmati Rice".into(),
            tax_id: Some(gst5.id),
            ..Default::default()
        };
        let item = store.add_item(rice.clone()).unwrap();
        assert_eq!(item.tax_name.as_deref(), Some("GST-5 (food)"));
        assert!(matches!(
            store.delete_tax(gst5.id),
            Err(StoreError::Invalid(_))
        ));
        rice.name = "Wheat".into();
        rice.tax_id = Some(999_999);
        assert!(matches!(store.add_item(rice), Err(StoreError::Invalid(_))));
        let unused = find_tax(&store, "IGST-3").unwrap().unwrap();
        store.delete_tax(unused.id).unwrap();
        assert!(matches!(store.tax(unused.id), Err(StoreError::Missing(_))));
    }
}
