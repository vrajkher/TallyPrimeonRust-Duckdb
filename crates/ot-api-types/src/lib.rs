//! Request and response types of the OpenTally `/api/v1` API.
//!
//! These types have no storage or server dependencies, so the WebAssembly UI and other
//! Rust clients can use them. The server's contract tests decode its real responses
//! into these types, so the two cannot drift apart unnoticed.

use serde::{Deserialize, Serialize};

/// Where the source code of this OpenTally is published (AGPL-3.0 section 13).
pub const SOURCE_URL: &str = "https://github.com/vrajkher/TallyPrimeonRust-Duckdb";

/// The body of every error response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiErrorBody {
    pub error: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct User {
    pub id: i64,
    pub username: String,
    pub full_name: String,
    pub enabled: bool,
    /// Role keys such as `system_manager` or `books_user`.
    pub roles: Vec<String>,
    pub created_at: String,
}

/// `GET /api/v1/session`, and the reply to sign-in and setup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionState {
    pub signed_in: bool,
    pub user: Option<User>,
    /// No users exist yet; create the first with `POST /api/v1/setup`.
    pub setup_required: bool,
    pub csrf_token: Option<String>,
    /// The company setup wizard has run.
    #[serde(default)]
    pub company_setup_complete: bool,
}

/// `GET /api/v1/company/defaults?country=…`: what the wizard suggests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CountryDefaults {
    pub country: String,
    pub currency: Option<String>,
    pub chart: String,
    pub fiscal_year_start: String,
    pub fiscal_year_end: String,
}

/// One chart template from `GET /api/v1/accounts/charts`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChartTemplate {
    pub name: String,
    pub country_code: String,
}

/// `POST /api/v1/company/setup`. Empty optional fields take the country's suggestions.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompanySetup {
    pub company_name: String,
    pub country: String,
    pub bank_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub currency: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chart: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fiscal_year_start: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fiscal_year_end: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gstin: Option<String>,
}

/// A group of `GET /api/v1/accounts`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupAccount {
    pub id: i64,
    pub name: String,
    pub parent_id: Option<i64>,
    pub root_type: Option<String>,
    /// `set`, `inherited` or `tally_name`.
    pub root_source: Option<String>,
    pub account_type: Option<String>,
}

/// A ledger of `GET /api/v1/accounts`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LedgerAccount {
    pub id: i64,
    pub name: String,
    pub kind: String,
    pub group_id: Option<i64>,
    pub root_type: Option<String>,
    pub account_type: Option<String>,
    pub effective_type: Option<String>,
    pub gst_head: Option<String>,
}

/// `GET /api/v1/accounts`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChartOfAccounts {
    pub groups: Vec<GroupAccount>,
    pub ledgers: Vec<LedgerAccount>,
    pub unclassified_ledgers: usize,
}

/// A number series from `GET /api/v1/series`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NumberSeries {
    pub prefix: String,
    /// The document type's key, e.g. `sales_invoice`.
    pub reference_type: String,
    pub reference_label: String,
    pub start: u64,
    pub pad_zeros: u8,
    pub current: u64,
    pub is_default: bool,
    pub next_name: String,
    pub used: bool,
}

/// A document type from `GET /api/v1/series/types`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeriesType {
    pub key: String,
    pub label: String,
    pub standard_prefix: String,
}

/// `POST /api/v1/series`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewNumberSeries {
    pub prefix: String,
    pub reference_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pad_zeros: Option<u8>,
    pub make_default: bool,
}

/// A party's address. `display` and `place_of_supply` are filled in by the server.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Address {
    pub address_line1: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address_line2: Option<String>,
    pub city: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    pub country: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub postal_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phone: Option<String>,
    #[serde(default, skip_serializing)]
    pub display: String,
    #[serde(default, skip_serializing)]
    pub place_of_supply: Option<String>,
}

/// A customer or supplier from `GET /api/v1/parties`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Party {
    pub id: i64,
    pub name: String,
    /// `customer`, `supplier` or `both`.
    pub role: String,
    pub email: Option<String>,
    pub phone: Option<String>,
    pub address: Option<Address>,
    pub default_ledger_id: Option<i64>,
    pub default_ledger_name: Option<String>,
    pub currency: String,
    pub tax_id: Option<String>,
    /// `unregistered`, `registered_regular` or `consumer`.
    pub gst_type: String,
    pub gstin: Option<String>,
    pub revision: i32,
}

/// `POST /api/v1/parties`, and `POST /api/v1/parties/{id}` with `revision` set.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PartyInput {
    pub name: String,
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phone: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<Address>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_ledger_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub currency: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tax_id: Option<String>,
    pub gst_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gstin: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revision: Option<i32>,
}

/// A unit of measure from `GET /api/v1/units`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Unit {
    pub name: String,
    /// Quantities in this unit must be whole numbers.
    pub is_whole: bool,
    #[serde(default)]
    pub used_by: i64,
}

/// An item group from `GET /api/v1/item-groups`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemGroup {
    pub id: i64,
    pub name: String,
    pub hsn_code: Option<String>,
    #[serde(default)]
    pub items: i64,
}

/// One other unit an item is traded in: one `unit` is `factor` of the item's unit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Conversion {
    pub unit: String,
    pub factor: String,
}

/// An item from `GET /api/v1/items`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Item {
    pub id: i64,
    pub name: String,
    pub item_code: Option<String>,
    pub group_id: Option<i64>,
    pub group_name: Option<String>,
    /// `purchases`, `sales` or `both`.
    pub usage: String,
    /// `product` or `service`.
    pub item_type: String,
    pub unit: String,
    /// A decimal amount in the company currency, e.g. `"125.50"`.
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

/// `POST /api/v1/items`, and `POST /api/v1/items/{id}` with `revision` set. Missing
/// accounts take the company defaults.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemInput {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group_id: Option<i64>,
    pub usage: String,
    pub item_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rate: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub income_ledger_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expense_ledger_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hsn_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub barcode: Option<String>,
    pub track_item: bool,
    pub conversions: Vec<Conversion>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revision: Option<i32>,
}

/// `POST /api/v1/session`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignIn {
    pub username: String,
    pub password: String,
}

/// `POST /api/v1/setup`: the first user, who becomes a System Manager.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Setup {
    pub username: String,
    pub full_name: String,
    pub password: String,
}

/// Query of `GET /api/v1/vouchers` (the Day Book). Empty fields are left out.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DayBookFilter {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub q: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page: Option<u32>,
}

impl DayBookFilter {
    /// The filter as a URL query string, e.g. `kind=payment&page=2`.
    pub fn query(&self) -> String {
        [
            ("kind", self.kind.clone()),
            ("from", self.from.clone()),
            ("to", self.to.clone()),
            ("q", self.q.clone()),
            ("status", self.status.clone()),
            ("page", self.page.map(|p| p.to_string())),
        ]
        .into_iter()
        .filter_map(|(key, value)| {
            value
                .filter(|v| !v.trim().is_empty())
                .map(|v| format!("{key}={}", encode(v.trim())))
        })
        .collect::<Vec<_>>()
        .join("&")
    }
}

/// One voucher row of the Day Book.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoucherRow {
    pub id: i64,
    pub kind: String,
    pub code: String,
    pub date: String,
    pub cash_account: String,
    pub other_account: String,
    pub debit: String,
    pub credit: String,
    /// Whole paise as a decimal string.
    pub amount_paise: String,
    pub reference: String,
    pub narration: String,
    pub type_name: String,
    pub status: String,
    pub revision: i32,
}

/// `GET /api/v1/vouchers`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DayBook {
    pub rows: Vec<VoucherRow>,
    pub page: u32,
    pub page_size: u32,
    pub count: i64,
    pub receipt_paise: String,
    pub payment_paise: String,
}

impl DayBook {
    pub fn pages(&self) -> u32 {
        let size = i64::from(self.page_size.max(1));
        u32::try_from(((self.count + size - 1) / size).max(1)).unwrap_or(u32::MAX)
    }
}

/// Paise as rupees with Indian digit grouping, e.g. `"12500050"` → `"1,25,000.50"`.
/// Returns the input unchanged if it is not a whole number.
pub fn format_paise(paise: &str) -> String {
    let Ok(value) = paise.trim().parse::<i128>() else {
        return paise.to_owned();
    };
    let sign = if value < 0 { "-" } else { "" };
    let value = value.unsigned_abs();
    let (rupees, paise) = (value / 100, value % 100);
    format!("{sign}{}.{paise:02}", group_indian(&rupees.to_string()))
}

fn group_indian(digits: &str) -> String {
    if digits.len() <= 3 {
        return digits.to_owned();
    }
    let (head, last3) = digits.split_at(digits.len() - 3);
    let mut groups = Vec::new();
    let mut rest = head;
    while rest.len() > 2 {
        let (left, right) = rest.split_at(rest.len() - 2);
        groups.push(right);
        rest = left;
    }
    if !rest.is_empty() {
        groups.push(rest);
    }
    groups.reverse();
    format!("{},{last3}", groups.join(","))
}

/// Percent-encodes a query value.
fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paise_use_indian_grouping() {
        assert_eq!(format_paise("0"), "0.00");
        assert_eq!(format_paise("25000"), "250.00");
        assert_eq!(format_paise("12345678"), "1,23,456.78");
        assert_eq!(format_paise("12500050"), "1,25,000.50");
        assert_eq!(format_paise("1000000000000"), "10,00,00,00,000.00");
        assert_eq!(format_paise("-99999"), "-999.99");
        assert_eq!(format_paise("abc"), "abc");
    }

    #[test]
    fn filter_builds_an_encoded_query() {
        let filter = DayBookFilter {
            kind: Some("payment".into()),
            q: Some("tea & coffee".into()),
            from: Some(" ".into()),
            page: Some(2),
            ..Default::default()
        };
        assert_eq!(filter.query(), "kind=payment&q=tea%20%26%20coffee&page=2");
        assert_eq!(DayBookFilter::default().query(), "");
    }

    #[test]
    fn day_book_counts_pages_and_ignores_extra_fields() {
        let book: DayBook = serde_json::from_value(serde_json::json!({
            "rows": [], "page": 1, "page_size": 50, "count": 101,
            "receipt_paise": "0", "payment_paise": "0", "extra": true
        }))
        .unwrap();
        assert_eq!(book.pages(), 3);
    }
}
