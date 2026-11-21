// Derived from Frappe Books (AGPL-3.0)
// frappe_books/frappe_books/doctype/books_payment_method/books_payment_method.py,
// frappe_books/accounting/accounts.py (validate_payment_account) and
// frappe_books/setup.py (the Cash and Bank methods) @953e9b4, modified: saves carry a
// revision.

//! Payment methods (roadmap step 1.8): Cash, Bank and transfer methods, each with
//! the ledger its payments go to.

use duckdb::{Connection, OptionalExt, params};
use ot_core::accounts::AccountType;
use serde::{Deserialize, Serialize};

use crate::{
    chart::chart_of,
    store::{Store, StoreError, required_text},
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MethodType {
    #[default]
    Cash,
    Bank,
    Transfer,
}

impl MethodType {
    pub fn key(self) -> &'static str {
        match self {
            Self::Cash => "cash",
            Self::Bank => "bank",
            Self::Transfer => "transfer",
        }
    }

    fn from_key(key: &str) -> Self {
        match key {
            "bank" => Self::Bank,
            "transfer" => Self::Transfer,
            _ => Self::Cash,
        }
    }

    /// Cash methods take cash accounts; other methods cash or bank, as the Books
    /// payment form offers (Frappe's validate_payment_account).
    fn account_types(self) -> &'static [AccountType] {
        match self {
            Self::Cash => &[AccountType::Cash],
            Self::Bank | Self::Transfer => &[AccountType::Cash, AccountType::Bank],
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct PaymentMethodInput {
    pub name: String,
    #[serde(default)]
    pub method_type: MethodType,
    #[serde(default)]
    pub ledger_id: Option<i64>,
    #[serde(default)]
    pub requires_clearance_date: bool,
    #[serde(default)]
    pub revision: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PaymentMethod {
    pub id: i64,
    pub name: String,
    pub method_type: MethodType,
    pub ledger_id: Option<i64>,
    pub ledger_name: Option<String>,
    pub requires_clearance_date: bool,
    pub revision: i32,
}

fn invalid(text: impl Into<String>) -> StoreError {
    StoreError::Invalid(text.into())
}

fn check_ledger(
    conn: &Connection,
    kind: MethodType,
    ledger_id: Option<i64>,
) -> Result<(), StoreError> {
    let Some(id) = ledger_id else { return Ok(()) };
    let chart = chart_of(conn)?;
    let ledger = chart
        .ledgers
        .iter()
        .find(|l| l.id == id)
        .ok_or_else(|| invalid("Choose an existing ledger."))?;
    let allowed = kind.account_types();
    if !ledger.effective_type.is_some_and(|t| allowed.contains(&t)) {
        let wanted = allowed
            .iter()
            .map(|t| t.label())
            .collect::<Vec<_>>()
            .join(" or ");
        return Err(invalid(format!(
            "Account must be of type {wanted}, but {} is not.",
            ledger.name
        )));
    }
    Ok(())
}

fn name_taken(conn: &Connection, name: &str, except: Option<i64>) -> Result<bool, StoreError> {
    Ok(conn.query_row(
        "SELECT COUNT(*) > 0 FROM payment_methods WHERE name_key = ? AND id <> ?",
        params![name.trim().to_lowercase(), except.unwrap_or(-1)],
        |row| row.get(0),
    )?)
}

const SELECT: &str = "
    SELECT m.id, m.name, m.method_type, m.ledger_id, l.name, m.requires_clearance_date, m.revision
    FROM payment_methods m LEFT JOIN ledgers l ON l.id = m.ledger_id";

fn read(row: &duckdb::Row<'_>) -> duckdb::Result<PaymentMethod> {
    Ok(PaymentMethod {
        id: row.get(0)?,
        name: row.get(1)?,
        method_type: MethodType::from_key(&row.get::<_, String>(2)?),
        ledger_id: row.get(3)?,
        ledger_name: row.get(4)?,
        requires_clearance_date: row.get(5)?,
        revision: row.get(6)?,
    })
}

fn get(conn: &Connection, id: i64) -> Result<PaymentMethod, StoreError> {
    conn.query_row(&format!("{SELECT} WHERE m.id = ?"), params![id], read)
        .optional()?
        .ok_or_else(|| StoreError::Missing("Payment method not found.".into()))
}

impl Store {
    pub fn payment_methods(&self) -> Result<Vec<PaymentMethod>, StoreError> {
        let mut statement = self
            .connection
            .prepare(&format!("{SELECT} ORDER BY m.name_key"))?;
        let rows = statement
            .query_map([], read)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn add_payment_method(
        &mut self,
        input: PaymentMethodInput,
    ) -> Result<PaymentMethod, StoreError> {
        let name = required_text(&input.name, "the payment method name", 140)?.to_owned();
        let tx = self.connection.transaction()?;
        check_ledger(&tx, input.method_type, input.ledger_id)?;
        if name_taken(&tx, &name, None)? {
            return Err(StoreError::Conflict(format!(
                "A payment method named {name} already exists."
            )));
        }
        let id: i64 = tx.query_row(
            "INSERT INTO payment_methods (name, name_key, method_type, ledger_id, requires_clearance_date)
             VALUES (?, ?, ?, ?, ?) RETURNING id",
            params![
                name,
                name.to_lowercase(),
                input.method_type.key(),
                input.ledger_id,
                input.requires_clearance_date
            ],
            |row| row.get(0),
        )?;
        let saved = get(&tx, id)?;
        tx.commit()?;
        Ok(saved)
    }

    /// Replaces a payment method's fields. `input.revision` must be its current one.
    pub fn update_payment_method(
        &mut self,
        id: i64,
        input: PaymentMethodInput,
    ) -> Result<PaymentMethod, StoreError> {
        let name = required_text(&input.name, "the payment method name", 140)?.to_owned();
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
        check_ledger(&tx, input.method_type, input.ledger_id)?;
        if name_taken(&tx, &name, Some(id))? {
            return Err(StoreError::Conflict(format!(
                "A payment method named {name} already exists."
            )));
        }
        tx.execute(
            "UPDATE payment_methods SET name = ?, name_key = ?, method_type = ?, ledger_id = ?,
                 requires_clearance_date = ?, revision = revision + 1
             WHERE id = ?",
            params![
                name,
                name.to_lowercase(),
                input.method_type.key(),
                input.ledger_id,
                input.requires_clearance_date,
                id
            ],
        )?;
        let saved = get(&tx, id)?;
        tx.commit()?;
        Ok(saved)
    }

    /// Deletes a payment method. Once payments exist (Level 2), a method they use
    /// will be kept.
    pub fn delete_payment_method(&mut self, id: i64) -> Result<(), StoreError> {
        let tx = self.connection.transaction()?;
        get(&tx, id)?;
        tx.execute("DELETE FROM payment_methods WHERE id = ?", params![id])?;
        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chart::CompanySetup;

    fn store() -> Store {
        let mut store = Store::open_in_memory().unwrap();
        let setup: CompanySetup = serde_json::from_value(serde_json::json!({
            "company_name": "Shree Traders", "country": "India", "bank_name": "HDFC Bank"
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

    #[test]
    fn cash_and_bank_methods_are_seeded() {
        let store = store();
        let methods: Vec<(String, MethodType)> = store
            .payment_methods()
            .unwrap()
            .into_iter()
            .map(|m| (m.name, m.method_type))
            .collect();
        assert_eq!(
            methods,
            [
                ("Bank".into(), MethodType::Bank),
                ("Cash".into(), MethodType::Cash)
            ]
        );
    }

    // Ported from test_account_suits_the_method_type.
    #[test]
    fn account_suits_the_method_type() {
        let mut store = store();
        let cash = ledger(&store, "Cash");
        let bank = ledger(&store, "HDFC Bank");
        let income = ledger(&store, "Sales");
        for (kind, account, message) in [
            (MethodType::Cash, bank, Some("must be of type Cash, but")),
            (
                MethodType::Bank,
                income,
                Some("must be of type Cash or Bank"),
            ),
            (MethodType::Cash, cash, None),
            (MethodType::Bank, bank, None),
            (MethodType::Bank, cash, None),
            (MethodType::Transfer, bank, None),
        ] {
            let input = PaymentMethodInput {
                name: format!("{kind:?} {account}"),
                method_type: kind,
                ledger_id: Some(account),
                ..Default::default()
            };
            match (store.add_payment_method(input), message) {
                (Err(StoreError::Invalid(text)), Some(message)) => {
                    assert!(text.contains(message), "{text}");
                }
                (Ok(method), None) => assert_eq!(method.ledger_id, Some(account)),
                (other, _) => panic!("{kind:?} {account}: {other:?}"),
            }
        }
    }

    #[test]
    fn changes_need_the_revision_and_unique_names() {
        let mut store = store();
        let bank = ledger(&store, "HDFC Bank");
        let upi = store
            .add_payment_method(PaymentMethodInput {
                name: "UPI".into(),
                method_type: MethodType::Transfer,
                ledger_id: Some(bank),
                requires_clearance_date: false,
                revision: None,
            })
            .unwrap();
        assert!(matches!(
            store.add_payment_method(PaymentMethodInput {
                name: "upi".into(),
                ..Default::default()
            }),
            Err(StoreError::Conflict(_))
        ));
        let mut change = PaymentMethodInput {
            name: "UPI / NEFT".into(),
            method_type: MethodType::Transfer,
            ledger_id: Some(bank),
            requires_clearance_date: true,
            revision: None,
        };
        assert!(matches!(
            store.update_payment_method(upi.id, change.clone()),
            Err(StoreError::Invalid(_))
        ));
        change.revision = Some(1);
        let saved = store.update_payment_method(upi.id, change.clone()).unwrap();
        assert!(saved.requires_clearance_date);
        assert_eq!(saved.ledger_name.as_deref(), Some("HDFC Bank"));
        assert!(matches!(
            store.update_payment_method(upi.id, change),
            Err(StoreError::Conflict(_))
        ));
        store.delete_payment_method(upi.id).unwrap();
        assert!(matches!(
            store.delete_payment_method(upi.id),
            Err(StoreError::Missing(_))
        ));
    }
}
