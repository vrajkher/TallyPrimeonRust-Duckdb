// Derived from Frappe Books (AGPL-3.0)
// frappe_books/frappe_books/doctype/books_party/books_party.py and
// frappe_books/frappe_books/doctype/books_address/books_address.py @953e9b4, modified:
// a party owns its one address, and saves are checked against a revision number.

//! Parties (customers and suppliers) and their addresses (roadmap step 1.5).

use duckdb::{Connection, OptionalExt, params};
use ot_core::{
    accounts::AccountType,
    money::Currency,
    regional::{AddressParts, address_display, place_of_supply, validate_gstin},
    setup::is_country,
};
use serde::{Deserialize, Serialize};

use crate::{
    chart::{chart_of, settings_of},
    store::{Store, StoreError, required_text, validated_text},
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PartyRole {
    Customer,
    Supplier,
    /// Frappe's default role.
    #[default]
    Both,
}

impl PartyRole {
    pub fn key(self) -> &'static str {
        match self {
            Self::Customer => "customer",
            Self::Supplier => "supplier",
            Self::Both => "both",
        }
    }

    fn from_key(key: &str) -> Option<Self> {
        [Self::Customer, Self::Supplier, Self::Both]
            .into_iter()
            .find(|role| role.key() == key)
    }

    /// The account types a party of this role may default to (Frappe's
    /// PARTY_ACCOUNT_TYPES).
    fn account_types(self) -> &'static [AccountType] {
        match self {
            Self::Customer => &[AccountType::Receivable],
            Self::Supplier => &[AccountType::Payable],
            Self::Both => &[AccountType::Receivable, AccountType::Payable],
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GstType {
    #[default]
    Unregistered,
    RegisteredRegular,
    Consumer,
}

impl GstType {
    pub fn key(self) -> &'static str {
        match self {
            Self::Unregistered => "unregistered",
            Self::RegisteredRegular => "registered_regular",
            Self::Consumer => "consumer",
        }
    }

    fn from_key(key: &str) -> Option<Self> {
        [Self::Unregistered, Self::RegisteredRegular, Self::Consumer]
            .into_iter()
            .find(|kind| kind.key() == key)
    }
}

/// An address as it is entered.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AddressInput {
    pub address_line1: String,
    #[serde(default)]
    pub address_line2: Option<String>,
    pub city: String,
    #[serde(default)]
    pub state: Option<String>,
    pub country: String,
    #[serde(default)]
    pub postal_code: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub phone: Option<String>,
}

/// A saved address with its display line and place of supply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Address {
    #[serde(flatten)]
    pub input: AddressInput,
    pub display: String,
    pub place_of_supply: Option<String>,
}

/// A party as it is entered; on a change, `revision` is the one the caller read.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PartyInput {
    pub name: String,
    #[serde(default)]
    pub role: PartyRole,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub phone: Option<String>,
    #[serde(default)]
    pub address: Option<AddressInput>,
    #[serde(default)]
    pub default_ledger_id: Option<i64>,
    #[serde(default)]
    pub currency: Option<String>,
    #[serde(default)]
    pub tax_id: Option<String>,
    #[serde(default)]
    pub gst_type: GstType,
    #[serde(default)]
    pub gstin: Option<String>,
    #[serde(default)]
    pub revision: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Party {
    pub id: i64,
    pub name: String,
    pub role: PartyRole,
    pub email: Option<String>,
    pub phone: Option<String>,
    pub address: Option<Address>,
    pub default_ledger_id: Option<i64>,
    pub default_ledger_name: Option<String>,
    pub currency: String,
    pub tax_id: Option<String>,
    pub gst_type: GstType,
    pub gstin: Option<String>,
    pub revision: i32,
}

/// `GET /api/v1/parties` filters.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PartyFilter {
    /// `customer` lists customers and parties with both roles; likewise `supplier`.
    #[serde(default)]
    pub role: Option<PartyRole>,
    /// Text in the name, email, phone or GSTIN.
    #[serde(default)]
    pub q: Option<String>,
}

fn optional(text: Option<&str>, label: &str, max: usize) -> Result<Option<String>, StoreError> {
    Ok(text
        .map(|t| validated_text(t, label, max))
        .transpose()?
        .filter(|t| !t.is_empty())
        .map(str::to_owned))
}

fn clean_email(email: Option<&str>) -> Result<Option<String>, StoreError> {
    let email = optional(email, "Email", 254)?;
    if let Some(email) = &email {
        let (local, domain) = email.split_once('@').unwrap_or_default();
        if local.is_empty() || !domain.contains('.') || email.contains(char::is_whitespace) {
            return Err(StoreError::Invalid(format!(
                "{email} is not a valid email address."
            )));
        }
    }
    Ok(email)
}

fn clean_address(input: &AddressInput) -> Result<Address, StoreError> {
    let country = required_text(&input.country, "the country", 100)?;
    if !is_country(country) {
        return Err(StoreError::Invalid(format!(
            "{country} is not a country OpenTally knows. Choose one from the list."
        )));
    }
    let input = AddressInput {
        address_line1: required_text(&input.address_line1, "the first address line", 140)?
            .to_owned(),
        address_line2: optional(input.address_line2.as_deref(), "Address line 2", 140)?,
        city: required_text(&input.city, "the city", 100)?.to_owned(),
        state: optional(input.state.as_deref(), "State", 100)?,
        country: country.to_owned(),
        postal_code: optional(input.postal_code.as_deref(), "Postal code", 20)?,
        email: clean_email(input.email.as_deref())?,
        phone: optional(input.phone.as_deref(), "Phone", 40)?,
    };
    let display = address_display(&AddressParts {
        line1: &input.address_line1,
        line2: input.address_line2.as_deref(),
        city: &input.city,
        state: input.state.as_deref(),
        country: &input.country,
        postal_code: input.postal_code.as_deref(),
    });
    let place_of_supply =
        place_of_supply(&input.country, input.state.as_deref()).map(str::to_owned);
    Ok(Address {
        input,
        display,
        place_of_supply,
    })
}

/// A party as it will be saved, after Frappe's before_validate and validate.
struct Clean {
    name: String,
    role: PartyRole,
    email: Option<String>,
    phone: Option<String>,
    address: Option<Address>,
    default_ledger_id: Option<i64>,
    currency: String,
    tax_id: Option<String>,
    gst_type: GstType,
    gstin: Option<String>,
}

fn clean(conn: &Connection, input: &PartyInput) -> Result<Clean, StoreError> {
    let name = required_text(&input.name, "the party name", 140)?.to_owned();
    let settings = settings_of(conn)?;
    let chart = chart_of(conn)?;
    let default_ledger_id = match input.default_ledger_id {
        Some(id) => {
            let ledger = chart
                .ledgers
                .iter()
                .find(|l| l.id == id)
                .ok_or_else(|| StoreError::Invalid("Choose an existing ledger.".into()))?;
            let allowed = input.role.account_types();
            if !ledger.effective_type.is_some_and(|t| allowed.contains(&t)) {
                let wanted = allowed
                    .iter()
                    .map(|t| format!("{t:?}"))
                    .collect::<Vec<_>>()
                    .join(" or ");
                return Err(StoreError::Invalid(format!(
                    "Default account {} must be of type {wanted}.",
                    ledger.name
                )));
            }
            Some(id)
        }
        // As Frappe's _default_account: the company's receivable or payable.
        None => match input.role {
            PartyRole::Customer => settings.receivable_ledger_id,
            PartyRole::Supplier => settings.payable_ledger_id,
            PartyRole::Both => None,
        },
    };
    let currency = match input.currency.as_deref().map(str::trim) {
        Some(code) if !code.is_empty() => Currency::new(code)
            .map_err(|e| StoreError::Invalid(e.to_string()))?
            .code()
            .to_owned(),
        _ => settings.currency.clone(),
    };
    let gstin = match input.gst_type {
        GstType::RegisteredRegular => match input.gstin.as_deref().map(str::trim) {
            Some(gstin) if !gstin.is_empty() => {
                Some(validate_gstin(gstin).map_err(|e| StoreError::Invalid(e.to_string()))?)
            }
            _ => {
                return Err(StoreError::Invalid(
                    "GSTIN is required for a registered party.".into(),
                ));
            }
        },
        // Frappe clears the GSTIN of a party that is not registered.
        _ => None,
    };
    Ok(Clean {
        name,
        role: input.role,
        email: clean_email(input.email.as_deref())?,
        phone: optional(input.phone.as_deref(), "Phone", 40)?,
        address: input.address.as_ref().map(clean_address).transpose()?,
        default_ledger_id,
        currency,
        tax_id: optional(input.tax_id.as_deref(), "Tax ID", 40)?,
        gst_type: input.gst_type,
        gstin,
    })
}

fn name_key(name: &str) -> String {
    name.trim().to_lowercase()
}

fn name_taken(conn: &Connection, name: &str, except: Option<i64>) -> Result<bool, StoreError> {
    Ok(conn.query_row(
        "SELECT COUNT(*) > 0 FROM parties WHERE name_key = ? AND id <> ?",
        params![name_key(name), except.unwrap_or(-1)],
        |row| row.get(0),
    )?)
}

fn save_address(
    conn: &Connection,
    existing: Option<i64>,
    address: &Option<Address>,
) -> Result<Option<i64>, StoreError> {
    let Some(address) = address else {
        if let Some(id) = existing {
            conn.execute("DELETE FROM addresses WHERE id = ?", params![id])?;
        }
        return Ok(None);
    };
    let a = &address.input;
    let values = params![
        a.address_line1,
        a.address_line2,
        a.city,
        a.state,
        a.country,
        a.postal_code,
        a.email,
        a.phone,
        address.display,
        address.place_of_supply,
    ];
    if let Some(id) = existing {
        conn.execute(
            "UPDATE addresses SET address_line1 = ?, address_line2 = ?, city = ?, state = ?,
                 country = ?, postal_code = ?, email = ?, phone = ?, display = ?,
                 place_of_supply = ?
             WHERE id = ?",
            params![
                a.address_line1,
                a.address_line2,
                a.city,
                a.state,
                a.country,
                a.postal_code,
                a.email,
                a.phone,
                address.display,
                address.place_of_supply,
                id
            ],
        )?;
        return Ok(Some(id));
    }
    let id = conn.query_row(
        "INSERT INTO addresses (address_line1, address_line2, city, state, country,
             postal_code, email, phone, display, place_of_supply)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
        values,
        |row| row.get(0),
    )?;
    Ok(Some(id))
}

const PARTY_SELECT: &str = "
    SELECT p.id, p.name, p.role, p.email, p.phone, p.default_ledger_id, l.name, p.currency,
           p.tax_id, p.gst_type, p.gstin, p.revision,
           a.id, a.address_line1, a.address_line2, a.city, a.state, a.country, a.postal_code,
           a.email, a.phone, a.display, a.place_of_supply
    FROM parties p
    LEFT JOIN ledgers l ON l.id = p.default_ledger_id
    LEFT JOIN addresses a ON a.id = p.address_id";

fn read_party(row: &duckdb::Row<'_>) -> duckdb::Result<Party> {
    let role: String = row.get(2)?;
    let gst_type: String = row.get(9)?;
    let address = match row.get::<_, Option<i64>>(12)? {
        Some(_) => Some(Address {
            input: AddressInput {
                address_line1: row.get(13)?,
                address_line2: row.get(14)?,
                city: row.get(15)?,
                state: row.get(16)?,
                country: row.get(17)?,
                postal_code: row.get(18)?,
                email: row.get(19)?,
                phone: row.get(20)?,
            },
            display: row.get(21)?,
            place_of_supply: row.get(22)?,
        }),
        None => None,
    };
    Ok(Party {
        id: row.get(0)?,
        name: row.get(1)?,
        // The CHECK constraints keep these to known keys.
        role: PartyRole::from_key(&role).unwrap_or(PartyRole::Both),
        email: row.get(3)?,
        phone: row.get(4)?,
        default_ledger_id: row.get(5)?,
        default_ledger_name: row.get(6)?,
        currency: row.get(7)?,
        tax_id: row.get(8)?,
        gst_type: GstType::from_key(&gst_type).unwrap_or_default(),
        gstin: row.get(10)?,
        revision: row.get(11)?,
        address,
    })
}

fn get(conn: &Connection, id: i64) -> Result<Party, StoreError> {
    conn.query_row(
        &format!("{PARTY_SELECT} WHERE p.id = ?"),
        params![id],
        read_party,
    )
    .optional()?
    .ok_or_else(|| StoreError::Missing("Party not found.".into()))
}

impl Store {
    pub fn parties(&self, filter: &PartyFilter) -> Result<Vec<Party>, StoreError> {
        let roles: Vec<&str> = match filter.role {
            None => vec!["customer", "supplier", "both"],
            Some(PartyRole::Both) => vec!["both"],
            Some(role) => vec![role.key(), "both"],
        };
        let q = filter
            .q
            .as_deref()
            .map(str::trim)
            .filter(|q| !q.is_empty())
            .map(|q| format!("%{}%", q.to_lowercase()));
        let mut statement = self.connection.prepare(&format!(
            "{PARTY_SELECT}
             WHERE p.role IN (?, ?, ?)
               AND (CAST(? AS VARCHAR) IS NULL OR p.name_key LIKE ?
                    OR lower(coalesce(p.email, '')) LIKE ?
                    OR lower(coalesce(p.phone, '')) LIKE ?
                    OR lower(coalesce(p.gstin, '')) LIKE ?)
             ORDER BY p.name_key"
        ))?;
        let role = |i: usize| roles.get(i).copied().unwrap_or(roles[0]);
        let rows = statement
            .query_map(
                params![role(0), role(1), role(2), q, q, q, q, q],
                read_party,
            )?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn party(&self, id: i64) -> Result<Party, StoreError> {
        get(&self.connection, id)
    }

    pub fn add_party(&mut self, input: PartyInput) -> Result<Party, StoreError> {
        let tx = self.connection.transaction()?;
        let party = clean(&tx, &input)?;
        if name_taken(&tx, &party.name, None)? {
            return Err(StoreError::Conflict(format!(
                "A party named {} already exists.",
                party.name
            )));
        }
        let address_id = save_address(&tx, None, &party.address)?;
        let id: i64 = tx.query_row(
            "INSERT INTO parties (name, name_key, role, email, phone, address_id,
                 default_ledger_id, currency, tax_id, gst_type, gstin)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
            params![
                party.name,
                name_key(&party.name),
                party.role.key(),
                party.email,
                party.phone,
                address_id,
                party.default_ledger_id,
                party.currency,
                party.tax_id,
                party.gst_type.key(),
                party.gstin,
            ],
            |row| row.get(0),
        )?;
        let saved = get(&tx, id)?;
        tx.commit()?;
        Ok(saved)
    }

    /// Replaces a party's fields. `input.revision` must be the party's current
    /// revision, so a save made from a stale copy is refused.
    pub fn update_party(&mut self, id: i64, input: PartyInput) -> Result<Party, StoreError> {
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
            None => {
                return Err(StoreError::Invalid(
                    "Send the revision you are changing.".into(),
                ));
            }
        }
        let party = clean(&tx, &input)?;
        if name_taken(&tx, &party.name, Some(id))? {
            return Err(StoreError::Conflict(format!(
                "A party named {} already exists.",
                party.name
            )));
        }
        let address_id: Option<i64> = tx.query_row(
            "SELECT address_id FROM parties WHERE id = ?",
            params![id],
            |row| row.get(0),
        )?;
        let address_id = save_address(&tx, address_id, &party.address)?;
        tx.execute(
            "UPDATE parties SET name = ?, name_key = ?, role = ?, email = ?, phone = ?,
                 address_id = ?, default_ledger_id = ?, currency = ?, tax_id = ?,
                 gst_type = ?, gstin = ?, revision = revision + 1
             WHERE id = ?",
            params![
                party.name,
                name_key(&party.name),
                party.role.key(),
                party.email,
                party.phone,
                address_id,
                party.default_ledger_id,
                party.currency,
                party.tax_id,
                party.gst_type.key(),
                party.gstin,
                id,
            ],
        )?;
        let saved = get(&tx, id)?;
        tx.commit()?;
        Ok(saved)
    }

    /// Deletes a party and its address. Once invoices and payments exist (Level 2),
    /// a party they name will be kept.
    pub fn delete_party(&mut self, id: i64) -> Result<(), StoreError> {
        let tx = self.connection.transaction()?;
        let address_id: Option<i64> = tx
            .query_row(
                "SELECT address_id FROM parties WHERE id = ?",
                params![id],
                |row| row.get(0),
            )
            .optional()?
            .ok_or_else(|| StoreError::Missing("Party not found.".into()))?;
        tx.execute("DELETE FROM parties WHERE id = ?", params![id])?;
        if let Some(address_id) = address_id {
            tx.execute("DELETE FROM addresses WHERE id = ?", params![address_id])?;
        }
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

    fn party(name: &str, role: PartyRole) -> PartyInput {
        PartyInput {
            name: name.into(),
            role,
            ..Default::default()
        }
    }

    fn ledger_of_type(store: &Store, kind: AccountType) -> i64 {
        store
            .chart_of_accounts()
            .unwrap()
            .ledgers
            .into_iter()
            .find(|l| l.effective_type == Some(kind))
            .unwrap()
            .id
    }

    // Ported from test_party_defaults_to_the_role_ledger_and_company_currency.
    #[test]
    fn party_defaults_to_the_role_ledger_and_company_currency() {
        let mut store = store();
        let settings = store.accounting_settings().unwrap();
        let customer = store
            .add_party(party("Asha Stores", PartyRole::Customer))
            .unwrap();
        assert_eq!(customer.default_ledger_id, settings.receivable_ledger_id);
        assert!(customer.default_ledger_id.is_some());
        assert_eq!(customer.currency, "INR");
        let supplier = store
            .add_party(party("Kiran Mills", PartyRole::Supplier))
            .unwrap();
        assert_eq!(supplier.default_ledger_id, settings.payable_ledger_id);
        let both = store
            .add_party(party("Mehta & Sons", PartyRole::Both))
            .unwrap();
        assert_eq!(both.default_ledger_id, None);
        assert_eq!(both.revision, 1);
    }

    // Ported from test_default_account_follows_the_role.
    #[test]
    fn default_account_follows_the_role() {
        let mut store = store();
        let receivable = ledger_of_type(&store, AccountType::Receivable);
        let payable = ledger_of_type(&store, AccountType::Payable);
        for (account, role, message) in [
            (payable, PartyRole::Customer, "must be of type Receivable"),
            (receivable, PartyRole::Supplier, "must be of type Payable"),
        ] {
            let mut input = party("Wrong", role);
            input.default_ledger_id = Some(account);
            match store.add_party(input) {
                Err(StoreError::Invalid(text)) => assert!(text.contains(message), "{text}"),
                other => panic!("{other:?}"),
            }
        }
        let mut both = party("Either", PartyRole::Both);
        both.default_ledger_id = Some(payable);
        assert_eq!(
            store.add_party(both).unwrap().default_ledger_id,
            Some(payable)
        );
        let cash = ledger_of_type(&store, AccountType::Cash);
        let mut cash_party = party("Cash Party", PartyRole::Both);
        cash_party.default_ledger_id = Some(cash);
        assert!(matches!(
            store.add_party(cash_party),
            Err(StoreError::Invalid(_))
        ));
    }

    // Ported from the three GSTIN tests of test_books_party.
    #[test]
    fn registered_parties_need_a_valid_gstin_others_have_none() {
        let mut store = store();
        let mut bad = party("GST Party", PartyRole::Customer);
        bad.gst_type = GstType::RegisteredRegular;
        bad.gstin = Some("invalid".into());
        assert!(matches!(
            store.add_party(bad.clone()),
            Err(StoreError::Invalid(_))
        ));
        bad.gstin = None;
        assert!(matches!(
            store.add_party(bad.clone()),
            Err(StoreError::Invalid(_))
        ));
        bad.gstin = Some(" 27aaaaa0000a1z5 ".into());
        assert_eq!(
            store.add_party(bad).unwrap().gstin.as_deref(),
            Some("27AAAAA0000A1Z5")
        );
        let mut unregistered = party("Walk-in", PartyRole::Customer);
        unregistered.gstin = Some("27AAAAA0000A1Z5".into());
        assert_eq!(store.add_party(unregistered).unwrap().gstin, None);
    }

    // Ported from test_address_display_and_indian_place_of_supply and
    // test_country_must_be_a_frappe_country.
    #[test]
    fn addresses_are_shown_on_one_line_with_the_place_of_supply() {
        let mut store = store();
        let address = AddressInput {
            address_line1: "42 Market Road".into(),
            city: "Mumbai".into(),
            state: Some("Maharashtra".into()),
            country: "India".into(),
            postal_code: Some("400001".into()),
            ..Default::default()
        };
        let mut input = party("Asha Stores", PartyRole::Customer);
        input.address = Some(address.clone());
        let saved = store.add_party(input).unwrap();
        let shown = saved.address.as_ref().unwrap();
        assert_eq!(
            shown.display,
            "42 Market Road, Mumbai, Maharashtra, India, 400001"
        );
        assert_eq!(shown.place_of_supply.as_deref(), Some("Maharashtra"));

        let mut narnia = party("Narnia Traders", PartyRole::Customer);
        narnia.address = Some(AddressInput {
            country: "Narnia".into(),
            ..address
        });
        assert!(matches!(
            store.add_party(narnia),
            Err(StoreError::Invalid(_))
        ));
    }

    // Ported in spirit from test_stale_party_save_cannot_reset_outstanding: a save from
    // a stale copy is refused.
    #[test]
    fn changes_need_the_current_revision_and_unique_names() {
        let mut store = store();
        let asha = store
            .add_party(party("Asha Stores", PartyRole::Customer))
            .unwrap();
        store
            .add_party(party("Kiran Mills", PartyRole::Supplier))
            .unwrap();
        assert!(matches!(
            store.add_party(party("ASHA STORES", PartyRole::Customer)),
            Err(StoreError::Conflict(_))
        ));

        let mut change = party("Asha Stores (Pune)", PartyRole::Customer);
        change.email = Some("accounts@asha.example".into());
        change.address = Some(AddressInput {
            address_line1: "7 FC Road".into(),
            city: "Pune".into(),
            state: Some("Maharashtra".into()),
            country: "India".into(),
            ..Default::default()
        });
        assert!(matches!(
            store.update_party(asha.id, change.clone()),
            Err(StoreError::Invalid(_))
        ));
        change.revision = Some(asha.revision);
        let saved = store.update_party(asha.id, change.clone()).unwrap();
        assert_eq!(saved.revision, 2);
        assert_eq!(saved.name, "Asha Stores (Pune)");
        assert!(saved.address.is_some());
        // The same stale revision again.
        assert!(matches!(
            store.update_party(asha.id, change.clone()),
            Err(StoreError::Conflict(_))
        ));
        let mut rename = party("kiran mills", PartyRole::Customer);
        rename.revision = Some(2);
        assert!(matches!(
            store.update_party(asha.id, rename),
            Err(StoreError::Conflict(_))
        ));
        // Removing the address deletes it.
        let mut no_address = change;
        no_address.address = None;
        no_address.revision = Some(2);
        assert!(
            store
                .update_party(asha.id, no_address)
                .unwrap()
                .address
                .is_none()
        );
        let addresses: i64 = store
            .connection
            .query_row("SELECT COUNT(*) FROM addresses", [], |r| r.get(0))
            .unwrap();
        assert_eq!(addresses, 0);

        let mut bad_email = party("Mail", PartyRole::Customer);
        bad_email.email = Some("not an email".into());
        assert!(matches!(
            store.add_party(bad_email),
            Err(StoreError::Invalid(_))
        ));
    }

    #[test]
    fn list_filters_by_role_and_text_and_delete_removes_the_address() {
        let mut store = store();
        let mut asha = party("Asha Stores", PartyRole::Customer);
        asha.address = Some(AddressInput {
            address_line1: "1 Main Road".into(),
            city: "Surat".into(),
            country: "India".into(),
            ..Default::default()
        });
        let asha = store.add_party(asha).unwrap();
        store
            .add_party(party("Kiran Mills", PartyRole::Supplier))
            .unwrap();
        let mut mehta = party("Mehta & Sons", PartyRole::Both);
        mehta.phone = Some("98250 12345".into());
        store.add_party(mehta).unwrap();

        let names = |filter: PartyFilter| -> Vec<String> {
            store
                .parties(&filter)
                .unwrap()
                .into_iter()
                .map(|p| p.name)
                .collect()
        };
        assert_eq!(names(PartyFilter::default()).len(), 3);
        assert_eq!(
            names(PartyFilter {
                role: Some(PartyRole::Customer),
                q: None
            }),
            ["Asha Stores", "Mehta & Sons"]
        );
        assert_eq!(
            names(PartyFilter {
                role: Some(PartyRole::Supplier),
                q: None
            }),
            ["Kiran Mills", "Mehta & Sons"]
        );
        assert_eq!(
            names(PartyFilter {
                role: None,
                q: Some("98250".into())
            }),
            ["Mehta & Sons"]
        );
        store.delete_party(asha.id).unwrap();
        assert!(matches!(store.party(asha.id), Err(StoreError::Missing(_))));
        assert!(matches!(
            store.delete_party(asha.id),
            Err(StoreError::Missing(_))
        ));
        let addresses: i64 = store
            .connection
            .query_row("SELECT COUNT(*) FROM addresses", [], |r| r.get(0))
            .unwrap();
        assert_eq!(addresses, 0);
    }
}
