// Derived from Frappe Books (AGPL-3.0)
// frappe_books/frappe_books/doctype/books_item/books_item.py, books_item_group,
// books_uom, frappe_books/regional.py (validate_hsn_codes), frappe_books/setup.py
// (DEFAULT_UOMS) and frappe_books/accounting/accounts.py (item_account_root_types)
// @953e9b4, modified: an item with no Cost of Goods Sold ledger defaults to a
// "Purchase" ledger, and a service with no "Service" ledger to "Sales", as Tally
// charts have those instead; batches and serial numbers come with inventory.

//! Items, item groups and units of measure (roadmap step 1.6).

use std::{collections::HashSet, str::FromStr};

use duckdb::{Connection, OptionalExt, params};
use ot_core::{
    accounts::{AccountType, RootType},
    money::{Currency, Decimal, Money},
};
use serde::{Deserialize, Serialize};

use crate::{
    chart::{chart_of, settings_of},
    store::{Store, StoreError, required_text, validated_text},
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemUsage {
    Purchases,
    Sales,
    #[default]
    Both,
}

impl ItemUsage {
    pub fn key(self) -> &'static str {
        match self {
            Self::Purchases => "purchases",
            Self::Sales => "sales",
            Self::Both => "both",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemType {
    #[default]
    Product,
    Service,
}

impl ItemType {
    pub fn key(self) -> &'static str {
        match self {
            Self::Product => "product",
            Self::Service => "service",
        }
    }
}

fn parse_key<T: for<'de> Deserialize<'de> + Default>(key: &str) -> T {
    serde_json::from_value(serde_json::Value::String(key.to_owned())).unwrap_or_default()
}

// ------------------------------------------------------------------ units

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Unit {
    pub name: String,
    pub is_whole: bool,
    /// How many items use the unit, as their own unit or a conversion.
    pub used_by: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NewUnit {
    pub name: String,
    #[serde(default)]
    pub is_whole: bool,
}

// ------------------------------------------------------------------ groups

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ItemGroup {
    pub id: i64,
    pub name: String,
    pub hsn_code: Option<String>,
    pub items: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ItemGroupInput {
    pub name: String,
    #[serde(default)]
    pub hsn_code: Option<String>,
}

// ------------------------------------------------------------------ items

/// One other unit an item is traded in: one `unit` is `factor` of the item's unit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Conversion {
    pub unit: String,
    /// A decimal number, e.g. "12" or "0.5".
    pub factor: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ItemInput {
    pub name: String,
    #[serde(default)]
    pub item_code: Option<String>,
    #[serde(default)]
    pub group_id: Option<i64>,
    #[serde(default)]
    pub usage: ItemUsage,
    #[serde(default)]
    pub item_type: ItemType,
    /// Defaults to "Unit".
    #[serde(default)]
    pub unit: Option<String>,
    /// A decimal amount in the company currency, e.g. "125.50".
    #[serde(default)]
    pub rate: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub income_ledger_id: Option<i64>,
    #[serde(default)]
    pub expense_ledger_id: Option<i64>,
    #[serde(default)]
    pub hsn_code: Option<String>,
    #[serde(default)]
    pub barcode: Option<String>,
    #[serde(default)]
    pub track_item: bool,
    #[serde(default)]
    pub conversions: Vec<Conversion>,
    #[serde(default)]
    pub revision: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Item {
    pub id: i64,
    pub name: String,
    pub item_code: Option<String>,
    pub group_id: Option<i64>,
    pub group_name: Option<String>,
    pub usage: ItemUsage,
    pub item_type: ItemType,
    pub unit: String,
    /// The rate in the company currency, e.g. "125.50".
    pub rate: String,
    pub description: Option<String>,
    pub income_ledger_id: i64,
    pub income_ledger_name: String,
    pub expense_ledger_id: i64,
    pub expense_ledger_name: String,
    pub hsn_code: Option<String>,
    pub barcode: Option<String>,
    pub track_item: bool,
    pub conversions: Vec<Conversion>,
    pub revision: i32,
}

/// `GET /api/v1/items` filters.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ItemFilter {
    #[serde(default)]
    pub group_id: Option<i64>,
    /// Text in the name, item code, HSN code or barcode.
    #[serde(default)]
    pub q: Option<String>,
}

fn invalid(text: impl Into<String>) -> StoreError {
    StoreError::Invalid(text.into())
}

fn optional(text: Option<&str>, label: &str, max: usize) -> Result<Option<String>, StoreError> {
    Ok(text
        .map(|t| validated_text(t, label, max))
        .transpose()?
        .filter(|t| !t.is_empty())
        .map(str::to_owned))
}

/// HSN/SAC is India's GST code, so only an Indian company's codes are checked: 4 to 8
/// digits.
fn check_hsn(conn: &Connection, code: Option<&str>) -> Result<Option<String>, StoreError> {
    let code = optional(code, "HSN/SAC code", 20)?;
    if let Some(code) = &code {
        let digits = (4..=8).contains(&code.len()) && code.bytes().all(|b| b.is_ascii_digit());
        if !digits && settings_of(conn)?.country.as_deref() == Some("India") {
            return Err(invalid("HSN/SAC code must contain between 4 and 8 digits."));
        }
    }
    Ok(code)
}

fn company_currency(conn: &Connection) -> Result<Currency, StoreError> {
    Currency::new(&settings_of(conn)?.currency).map_err(|e| invalid(e.to_string()))
}

fn unit_name(conn: &Connection, name: &str) -> Result<String, StoreError> {
    conn.query_row(
        "SELECT name FROM units WHERE name_key = ?",
        params![name.trim().to_lowercase()],
        |row| row.get(0),
    )
    .optional()?
    .ok_or_else(|| {
        invalid(format!(
            "{} is not a unit. Add it to the units first.",
            name.trim()
        ))
    })
}

/// An item as it will be saved, after Frappe's before_validate and validate.
struct Clean {
    name: String,
    item_code: Option<String>,
    group_id: Option<i64>,
    usage: ItemUsage,
    item_type: ItemType,
    unit: String,
    rate_minor: i64,
    description: Option<String>,
    income_ledger_id: i64,
    expense_ledger_id: i64,
    hsn_code: Option<String>,
    barcode: Option<String>,
    track_item: bool,
    conversions: Vec<Conversion>,
}

fn clean(conn: &Connection, input: &ItemInput) -> Result<Clean, StoreError> {
    let name = required_text(&input.name, "the item name", 140)?.to_owned();
    if input.track_item && input.item_type != ItemType::Product {
        return Err(invalid("Only products can track inventory."));
    }
    let group_hsn = match input.group_id {
        Some(id) => Some(
            conn.query_row(
                "SELECT hsn_code FROM item_groups WHERE id = ?",
                params![id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()?
            .ok_or_else(|| invalid("Choose an existing item group."))?,
        ),
        None => None,
    };
    // As Frappe's fetch_from: an item without its own HSN code takes its group's.
    let hsn_code = check_hsn(conn, input.hsn_code.as_deref())?.or(group_hsn.flatten());
    let barcode = optional(input.barcode.as_deref(), "Barcode", 12)?;
    if let Some(code) = &barcode
        && !(code.len() == 12 && code.bytes().all(|b| b.is_ascii_digit()))
    {
        return Err(invalid("Barcode must contain exactly 12 digits."));
    }
    let currency = company_currency(conn)?;
    let rate = match input.rate.as_deref().map(str::trim) {
        Some(text) if !text.is_empty() => currency
            .parse(text)
            .map_err(|_| invalid(format!("{text} is not an amount in {}.", currency.code())))?,
        _ => Money::zero(currency),
    };
    if rate.is_negative() {
        return Err(invalid("Rate cannot be negative."));
    }
    let rate_minor = i64::try_from(rate.minor()).map_err(|_| invalid("The rate is too large."))?;

    let unit = unit_name(conn, input.unit.as_deref().unwrap_or("Unit"))?;
    let mut seen = HashSet::new();
    let mut conversions = Vec::with_capacity(input.conversions.len());
    for row in &input.conversions {
        let other = unit_name(conn, &row.unit)?;
        if !seen.insert(other.clone()) {
            return Err(invalid("Each unit can have only one conversion factor."));
        }
        let factor = Decimal::from_str(row.factor.trim())
            .ok()
            .filter(|f| {
                *f > Decimal::ZERO && f.scale() <= 6 && *f < Decimal::from(1_000_000_000_000_i64)
            })
            .ok_or_else(|| invalid("Conversion factors must be greater than zero."))?;
        conversions.push(Conversion {
            unit: other,
            factor: factor.normalize().to_string(),
        });
    }

    let chart = chart_of(conn)?;
    let ledger_named = |name: &str| {
        chart
            .ledgers
            .iter()
            .find(|l| l.name.eq_ignore_ascii_case(name))
            .map(|l| l.id)
    };
    let income_ledger_id = match input.income_ledger_id {
        Some(id) => id,
        // Frappe: products sell into Sales and services into Service.
        None => match input.item_type {
            ItemType::Product => ledger_named("Sales"),
            ItemType::Service => ledger_named("Service").or_else(|| ledger_named("Sales")),
        }
        .ok_or_else(|| invalid("Choose the sales account for this item."))?,
    };
    let expense_ledger_id = match input.expense_ledger_id {
        Some(id) => id,
        // Frappe: the newest Cost of Goods Sold ledger (the highest id here).
        None => chart
            .ledgers
            .iter()
            .filter(|l| l.effective_type == Some(AccountType::CostOfGoodsSold))
            .map(|l| l.id)
            .max()
            .or_else(|| ledger_named("Purchase"))
            .ok_or_else(|| invalid("Choose the purchase account for this item."))?,
    };
    // As Frappe's item_account_root_types: items sell into income; a tracked item is
    // bought into stock received but not billed, a liability.
    for (id, label, root) in [
        (income_ledger_id, "Sales account", RootType::Income),
        (
            expense_ledger_id,
            "Purchase account",
            if input.track_item {
                RootType::Liability
            } else {
                RootType::Expense
            },
        ),
    ] {
        let ledger = chart
            .ledgers
            .iter()
            .find(|l| l.id == id)
            .ok_or_else(|| invalid(format!("Choose an existing ledger for the {label}.")))?;
        if ledger.root_type != Some(root) {
            return Err(invalid(format!(
                "{label} must be of type {root:?}; {} is not.",
                ledger.name
            )));
        }
    }

    Ok(Clean {
        name,
        item_code: optional(input.item_code.as_deref(), "Item code", 140)?,
        group_id: input.group_id,
        usage: input.usage,
        item_type: input.item_type,
        unit,
        rate_minor,
        description: optional(input.description.as_deref(), "Description", 2000)?,
        income_ledger_id,
        expense_ledger_id,
        hsn_code,
        barcode,
        track_item: input.track_item,
        conversions,
    })
}

fn name_taken(
    conn: &Connection,
    table: &str,
    name: &str,
    except: Option<i64>,
) -> Result<bool, StoreError> {
    Ok(conn.query_row(
        &format!("SELECT COUNT(*) > 0 FROM {table} WHERE name_key = ? AND id <> ?"),
        params![name.trim().to_lowercase(), except.unwrap_or(-1)],
        |row| row.get(0),
    )?)
}

const ITEM_SELECT: &str = "
    SELECT i.id, i.name, i.item_code, i.group_id, g.name, i.item_usage, i.item_type, i.unit,
           i.rate_minor, i.description, i.income_ledger_id, il.name, i.expense_ledger_id,
           el.name, i.hsn_code, i.barcode, i.track_item, i.revision
    FROM items i
    LEFT JOIN item_groups g ON g.id = i.group_id
    LEFT JOIN ledgers il ON il.id = i.income_ledger_id
    LEFT JOIN ledgers el ON el.id = i.expense_ledger_id";

fn read_items(
    conn: &Connection,
    sql: &str,
    args: &[&dyn duckdb::ToSql],
) -> Result<Vec<Item>, StoreError> {
    let currency = company_currency(conn)?;
    let mut statement = conn.prepare(sql)?;
    let mut items = statement
        .query_map(args, |row| {
            Ok(Item {
                id: row.get(0)?,
                name: row.get(1)?,
                item_code: row.get(2)?,
                group_id: row.get(3)?,
                group_name: row.get(4)?,
                usage: parse_key(&row.get::<_, String>(5)?),
                item_type: parse_key(&row.get::<_, String>(6)?),
                unit: row.get(7)?,
                rate: row.get::<_, i64>(8)?.to_string(),
                description: row.get(9)?,
                income_ledger_id: row.get(10)?,
                income_ledger_name: row.get::<_, Option<String>>(11)?.unwrap_or_default(),
                expense_ledger_id: row.get(12)?,
                expense_ledger_name: row.get::<_, Option<String>>(13)?.unwrap_or_default(),
                hsn_code: row.get(14)?,
                barcode: row.get(15)?,
                track_item: row.get(16)?,
                revision: row.get(17)?,
                conversions: Vec::new(),
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut conversions = conn.prepare(
        "SELECT unit, CAST(conversion_factor AS VARCHAR) FROM item_units WHERE item_id = ? ORDER BY unit",
    )?;
    for item in &mut items {
        // Stored as minor units; shown as a decimal amount.
        let minor: i128 = item.rate.parse().unwrap_or(0);
        item.rate = Money::from_minor(minor, currency.clone())
            .map(|m| m.to_decimal().to_string())
            .unwrap_or_default();
        item.conversions = conversions
            .query_map(params![item.id], |row| {
                let factor: String = row.get(1)?;
                Ok(Conversion {
                    unit: row.get(0)?,
                    factor: Decimal::from_str(&factor)
                        .map(|f| f.normalize().to_string())
                        .unwrap_or(factor),
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
    }
    Ok(items)
}

fn get_item(conn: &Connection, id: i64) -> Result<Item, StoreError> {
    read_items(conn, &format!("{ITEM_SELECT} WHERE i.id = ?"), &[&id])?
        .pop()
        .ok_or_else(|| StoreError::Missing("Item not found.".into()))
}

fn save_conversions(conn: &Connection, id: i64, rows: &[Conversion]) -> Result<(), StoreError> {
    conn.execute("DELETE FROM item_units WHERE item_id = ?", params![id])?;
    for row in rows {
        conn.execute(
            "INSERT INTO item_units (item_id, unit, conversion_factor) VALUES (?, ?, CAST(? AS DECIMAL(18, 6)))",
            params![id, row.unit, row.factor],
        )?;
    }
    Ok(())
}

impl Store {
    // -------------------------------------------------------------- units

    pub fn units(&self) -> Result<Vec<Unit>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT u.name, u.is_whole,
                    (SELECT COUNT(*) FROM items i WHERE i.unit = u.name)
                    + (SELECT COUNT(*) FROM item_units c WHERE c.unit = u.name)
             FROM units u ORDER BY u.name_key",
        )?;
        let rows = statement
            .query_map([], |row| {
                Ok(Unit {
                    name: row.get(0)?,
                    is_whole: row.get(1)?,
                    used_by: row.get(2)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn add_unit(&mut self, input: NewUnit) -> Result<Unit, StoreError> {
        let name = required_text(&input.name, "the unit name", 40)?.to_owned();
        let exists: bool = self.connection.query_row(
            "SELECT COUNT(*) > 0 FROM units WHERE name_key = ?",
            params![name.to_lowercase()],
            |row| row.get(0),
        )?;
        if exists {
            return Err(StoreError::Conflict(format!(
                "The unit {name} already exists."
            )));
        }
        self.connection.execute(
            "INSERT INTO units (name, name_key, is_whole) VALUES (?, ?, ?)",
            params![name, name.to_lowercase(), input.is_whole],
        )?;
        Ok(Unit {
            name,
            is_whole: input.is_whole,
            used_by: 0,
        })
    }

    /// Deletes a unit no item uses.
    pub fn delete_unit(&mut self, name: &str) -> Result<(), StoreError> {
        let unit = self
            .units()?
            .into_iter()
            .find(|u| u.name.eq_ignore_ascii_case(name.trim()))
            .ok_or_else(|| StoreError::Missing(format!("Unit {name} not found.")))?;
        if unit.used_by > 0 {
            return Err(invalid(format!(
                "{} is used by {} item(s), so it cannot be deleted.",
                unit.name, unit.used_by
            )));
        }
        self.connection
            .execute("DELETE FROM units WHERE name = ?", params![unit.name])?;
        Ok(())
    }

    // -------------------------------------------------------------- groups

    pub fn item_groups(&self) -> Result<Vec<ItemGroup>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT g.id, g.name, g.hsn_code, (SELECT COUNT(*) FROM items i WHERE i.group_id = g.id)
             FROM item_groups g ORDER BY g.name_key",
        )?;
        let rows = statement
            .query_map([], |row| {
                Ok(ItemGroup {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    hsn_code: row.get(2)?,
                    items: row.get(3)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    fn item_group(&self, id: i64) -> Result<ItemGroup, StoreError> {
        self.item_groups()?
            .into_iter()
            .find(|g| g.id == id)
            .ok_or_else(|| StoreError::Missing("Item group not found.".into()))
    }

    pub fn add_item_group(&mut self, input: ItemGroupInput) -> Result<ItemGroup, StoreError> {
        let name = required_text(&input.name, "the item group name", 140)?.to_owned();
        let hsn = check_hsn(&self.connection, input.hsn_code.as_deref())?;
        if name_taken(&self.connection, "item_groups", &name, None)? {
            return Err(StoreError::Conflict(format!(
                "An item group named {name} already exists."
            )));
        }
        let id: i64 = self.connection.query_row(
            "INSERT INTO item_groups (name, name_key, hsn_code) VALUES (?, ?, ?) RETURNING id",
            params![name, name.to_lowercase(), hsn],
            |row| row.get(0),
        )?;
        self.item_group(id)
    }

    pub fn update_item_group(
        &mut self,
        id: i64,
        input: ItemGroupInput,
    ) -> Result<ItemGroup, StoreError> {
        self.item_group(id)?;
        let name = required_text(&input.name, "the item group name", 140)?.to_owned();
        let hsn = check_hsn(&self.connection, input.hsn_code.as_deref())?;
        if name_taken(&self.connection, "item_groups", &name, Some(id))? {
            return Err(StoreError::Conflict(format!(
                "An item group named {name} already exists."
            )));
        }
        self.connection.execute(
            "UPDATE item_groups SET name = ?, name_key = ?, hsn_code = ? WHERE id = ?",
            params![name, name.to_lowercase(), hsn, id],
        )?;
        self.item_group(id)
    }

    /// Deletes an item group that holds no items.
    pub fn delete_item_group(&mut self, id: i64) -> Result<(), StoreError> {
        let group = self.item_group(id)?;
        if group.items > 0 {
            return Err(invalid(format!(
                "{} holds {} item(s); move them to another group first.",
                group.name, group.items
            )));
        }
        self.connection
            .execute("DELETE FROM item_groups WHERE id = ?", params![id])?;
        Ok(())
    }

    // -------------------------------------------------------------- items

    pub fn items(&self, filter: &ItemFilter) -> Result<Vec<Item>, StoreError> {
        let q = filter
            .q
            .as_deref()
            .map(str::trim)
            .filter(|q| !q.is_empty())
            .map(|q| format!("%{}%", q.to_lowercase()));
        read_items(
            &self.connection,
            &format!(
                "{ITEM_SELECT}
                 WHERE (CAST(? AS BIGINT) IS NULL OR i.group_id = ?)
                   AND (CAST(? AS VARCHAR) IS NULL OR i.name_key LIKE ?
                        OR lower(coalesce(i.item_code, '')) LIKE ?
                        OR coalesce(i.hsn_code, '') LIKE ?
                        OR coalesce(i.barcode, '') LIKE ?)
                 ORDER BY i.name_key"
            ),
            &[&filter.group_id, &filter.group_id, &q, &q, &q, &q, &q],
        )
    }

    pub fn item(&self, id: i64) -> Result<Item, StoreError> {
        get_item(&self.connection, id)
    }

    pub fn add_item(&mut self, input: ItemInput) -> Result<Item, StoreError> {
        let tx = self.connection.transaction()?;
        let item = clean(&tx, &input)?;
        if name_taken(&tx, "items", &item.name, None)? {
            return Err(StoreError::Conflict(format!(
                "An item named {} already exists.",
                item.name
            )));
        }
        let id: i64 = tx.query_row(
            "INSERT INTO items (name, name_key, item_code, group_id, item_usage, item_type, unit,
                 rate_minor, description, income_ledger_id, expense_ledger_id, hsn_code, barcode,
                 track_item)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
            params![
                item.name,
                item.name.to_lowercase(),
                item.item_code,
                item.group_id,
                item.usage.key(),
                item.item_type.key(),
                item.unit,
                item.rate_minor,
                item.description,
                item.income_ledger_id,
                item.expense_ledger_id,
                item.hsn_code,
                item.barcode,
                item.track_item,
            ],
            |row| row.get(0),
        )?;
        save_conversions(&tx, id, &item.conversions)?;
        let saved = get_item(&tx, id)?;
        tx.commit()?;
        Ok(saved)
    }

    /// Replaces an item's fields. `input.revision` must be the item's current revision.
    pub fn update_item(&mut self, id: i64, input: ItemInput) -> Result<Item, StoreError> {
        let tx = self.connection.transaction()?;
        let current = get_item(&tx, id)?;
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
        let item = clean(&tx, &input)?;
        if name_taken(&tx, "items", &item.name, Some(id))? {
            return Err(StoreError::Conflict(format!(
                "An item named {} already exists.",
                item.name
            )));
        }
        tx.execute(
            "UPDATE items SET name = ?, name_key = ?, item_code = ?, group_id = ?, item_usage = ?,
                 item_type = ?, unit = ?, rate_minor = ?, description = ?, income_ledger_id = ?,
                 expense_ledger_id = ?, hsn_code = ?, barcode = ?, track_item = ?,
                 revision = revision + 1
             WHERE id = ?",
            params![
                item.name,
                item.name.to_lowercase(),
                item.item_code,
                item.group_id,
                item.usage.key(),
                item.item_type.key(),
                item.unit,
                item.rate_minor,
                item.description,
                item.income_ledger_id,
                item.expense_ledger_id,
                item.hsn_code,
                item.barcode,
                item.track_item,
                id,
            ],
        )?;
        save_conversions(&tx, id, &item.conversions)?;
        let saved = get_item(&tx, id)?;
        tx.commit()?;
        Ok(saved)
    }

    /// Deletes an item. Once invoices exist (Level 2), an item they use will be kept.
    pub fn delete_item(&mut self, id: i64) -> Result<(), StoreError> {
        let tx = self.connection.transaction()?;
        get_item(&tx, id)?;
        tx.execute("DELETE FROM item_units WHERE item_id = ?", params![id])?;
        tx.execute("DELETE FROM items WHERE id = ?", params![id])?;
        tx.commit()?;
        Ok(())
    }
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

    fn item(name: &str) -> ItemInput {
        ItemInput {
            name: name.into(),
            ..Default::default()
        }
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

    #[test]
    fn frappe_default_units_are_seeded() {
        let store = store("India");
        let units = store.units().unwrap();
        let names: Vec<&str> = units.iter().map(|u| u.name.as_str()).collect();
        assert_eq!(names, ["Day", "Gram", "Hour", "Kg", "Meter", "Unit"]);
        assert!(units.iter().find(|u| u.name == "Unit").unwrap().is_whole);
    }

    // Ported from test_validates_hsn_barcode_and_rate.
    #[test]
    fn validates_hsn_barcode_and_rate() {
        let mut store = store("India");
        for (field, value) in [("hsn", "12A4"), ("barcode", "123"), ("rate", "-1")] {
            let mut input = item("Bad");
            match field {
                "hsn" => input.hsn_code = Some(value.into()),
                "barcode" => input.barcode = Some(value.into()),
                _ => input.rate = Some(value.into()),
            }
            assert!(
                matches!(store.add_item(input), Err(StoreError::Invalid(_))),
                "{field}"
            );
        }
        let mut good = item("Basmati Rice");
        good.hsn_code = Some("100630".into());
        good.barcode = Some("123456789012".into());
        good.rate = Some("125.5".into());
        let saved = store.add_item(good).unwrap();
        assert_eq!(saved.hsn_code.as_deref(), Some("100630"));
        assert_eq!(saved.rate, "125.50");
        assert_eq!(saved.unit, "Unit");
        let mut paise = item("Too precise");
        paise.rate = Some("1.005".into());
        assert!(matches!(store.add_item(paise), Err(StoreError::Invalid(_))));
    }

    // Ported from test_item_group_hsn_code_must_contain_four_to_eight_digits and
    // test_hsn_code_digits_are_checked_only_for_an_indian_company.
    #[test]
    fn hsn_codes_are_checked_only_for_an_indian_company() {
        let mut india = store("India");
        let bad = ItemGroupInput {
            name: "Grains".into(),
            hsn_code: Some("12A4".into()),
        };
        match india.add_item_group(bad.clone()) {
            Err(StoreError::Invalid(text)) => assert!(text.contains("between 4 and 8 digits")),
            other => panic!("{other:?}"),
        }
        let mut us = store("United States");
        assert_eq!(
            us.add_item_group(bad).unwrap().hsn_code.as_deref(),
            Some("12A4")
        );
        let mut long = item("Widget");
        long.hsn_code = Some("1234567890".into());
        assert_eq!(
            us.add_item(long).unwrap().hsn_code.as_deref(),
            Some("1234567890")
        );
    }

    // Ported from test_missing_accounts_and_hsn_code_get_the_app_defaults.
    #[test]
    fn missing_accounts_and_hsn_code_get_the_defaults() {
        let mut store = store("India");
        let group = store
            .add_item_group(ItemGroupInput {
                name: "Consulting".into(),
                hsn_code: Some("998877".into()),
            })
            .unwrap();
        let mut service = item("Audit");
        service.item_type = ItemType::Service;
        service.group_id = Some(group.id);
        let service = store.add_item(service).unwrap();
        // Tally charts have no Service ledger, so services sell into Sales too.
        assert_eq!(service.income_ledger_name, "Sales");
        assert_eq!(service.expense_ledger_name, "Purchase");
        assert_eq!(service.hsn_code.as_deref(), Some("998877"));
        assert_eq!(service.group_name.as_deref(), Some("Consulting"));
        assert_eq!(store.item_groups().unwrap()[0].items, 1);
    }

    #[test]
    fn standard_chart_defaults_follow_frappe() {
        let mut store = store("United States");
        let product = store.add_item(item("Widget")).unwrap();
        assert_eq!(product.income_ledger_name, "Sales");
        assert_eq!(product.expense_ledger_name, "Cost of Goods Sold");
    }

    // Ported from test_accounts_follow_the_item_tracking.
    #[test]
    fn accounts_follow_the_item_tracking() {
        let mut store = store("India");
        let sales = ledger(&store, "Sales");
        let purchase = ledger(&store, "Purchase");
        let debtors = ledger(&store, "Debtors");
        let creditors = ledger(&store, "Creditors");
        for (income, expense, track, message) in [
            (
                purchase,
                purchase,
                false,
                "Sales account must be of type Income",
            ),
            (
                sales,
                creditors,
                false,
                "Purchase account must be of type Expense",
            ),
            (
                sales,
                purchase,
                true,
                "Purchase account must be of type Liability",
            ),
            (
                debtors,
                purchase,
                false,
                "Sales account must be of type Income",
            ),
        ] {
            let mut input = item("Wrong");
            input.income_ledger_id = Some(income);
            input.expense_ledger_id = Some(expense);
            input.track_item = track;
            match store.add_item(input) {
                Err(StoreError::Invalid(text)) => assert!(text.contains(message), "{text}"),
                other => panic!("{other:?}"),
            }
        }
        let mut tracked = item("Tracked");
        tracked.income_ledger_id = Some(sales);
        tracked.expense_ledger_id = Some(creditors);
        tracked.track_item = true;
        assert!(store.add_item(tracked).unwrap().track_item);
        let mut service = item("Tracked service");
        service.item_type = ItemType::Service;
        service.track_item = true;
        assert!(matches!(
            store.add_item(service),
            Err(StoreError::Invalid(_))
        ));
    }

    #[test]
    fn unit_conversions_are_unique_positive_and_known() {
        let mut store = store("India");
        let conversion = |unit: &str, factor: &str| Conversion {
            unit: unit.into(),
            factor: factor.into(),
        };
        let mut input = item("Basmati Rice");
        input.unit = Some("kg".into());
        input.conversions = vec![conversion("Gram", "0.001"), conversion("gram", "0.002")];
        match store.add_item(input.clone()) {
            Err(StoreError::Invalid(text)) => assert!(text.contains("only one conversion")),
            other => panic!("{other:?}"),
        }
        input.conversions = vec![conversion("Gram", "0")];
        assert!(matches!(
            store.add_item(input.clone()),
            Err(StoreError::Invalid(_))
        ));
        input.conversions = vec![conversion("Bag", "25")];
        assert!(matches!(
            store.add_item(input.clone()),
            Err(StoreError::Invalid(_))
        ));
        store
            .add_unit(NewUnit {
                name: "Bag".into(),
                is_whole: true,
            })
            .unwrap();
        input.conversions = vec![conversion("Bag", "25.000"), conversion("Gram", "0.001")];
        let saved = store.add_item(input).unwrap();
        assert_eq!(saved.unit, "Kg");
        assert_eq!(
            saved.conversions,
            [conversion("Bag", "25"), conversion("Gram", "0.001")]
        );
        // Used units stay.
        assert!(matches!(
            store.delete_unit("Bag"),
            Err(StoreError::Invalid(_))
        ));
        assert!(matches!(
            store.add_unit(NewUnit {
                name: "BAG".into(),
                is_whole: false
            }),
            Err(StoreError::Conflict(_))
        ));
        store.delete_unit("Day").unwrap();
    }

    #[test]
    fn changes_need_the_current_revision_and_groups_must_be_empty_to_go() {
        let mut store = store("India");
        let group = store
            .add_item_group(ItemGroupInput {
                name: "Grains".into(),
                hsn_code: None,
            })
            .unwrap();
        let mut input = item("Basmati Rice");
        input.group_id = Some(group.id);
        let rice = store.add_item(input.clone()).unwrap();
        assert!(matches!(
            store.add_item(item("BASMATI RICE")),
            Err(StoreError::Conflict(_))
        ));
        input.rate = Some("90".into());
        assert!(matches!(
            store.update_item(rice.id, input.clone()),
            Err(StoreError::Invalid(_))
        ));
        input.revision = Some(1);
        let saved = store.update_item(rice.id, input.clone()).unwrap();
        assert_eq!((saved.rate.as_str(), saved.revision), ("90.00", 2));
        assert!(matches!(
            store.update_item(rice.id, input),
            Err(StoreError::Conflict(_))
        ));
        assert!(matches!(
            store.delete_item_group(group.id),
            Err(StoreError::Invalid(_))
        ));
        store.delete_item(rice.id).unwrap();
        store.delete_item_group(group.id).unwrap();
        assert!(matches!(store.item(rice.id), Err(StoreError::Missing(_))));
        let filtered = store
            .items(&ItemFilter {
                group_id: None,
                q: Some("rice".into()),
            })
            .unwrap();
        assert!(filtered.is_empty());
    }
}
