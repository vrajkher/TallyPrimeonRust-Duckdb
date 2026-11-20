// Derived from Frappe Books (AGPL-3.0)
// frappe_books/frappe_books/doctype/books_account/books_account.py, frappe_books/coa.py
// and frappe_books/data/charts/*.json @953e9b4, modified: Rust types, and a chart of
// Tally's 28 reserved groups added alongside Frappe's charts.

//! The chart of accounts: root types, account types, the rules between them, and the
//! chart templates a new book can start from.
//!
//! In OpenTally a group account is an account group and a ledger account is a ledger,
//! as in Tally; Frappe Books' single account tree maps onto them one to one.

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The five root types. Every account belongs to one, through its top-level group.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RootType {
    Asset,
    Liability,
    Equity,
    Income,
    Expense,
}

impl RootType {
    pub const ALL: [Self; 5] = [
        Self::Asset,
        Self::Liability,
        Self::Equity,
        Self::Income,
        Self::Expense,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Asset => "Asset",
            Self::Liability => "Liability",
            Self::Equity => "Equity",
            Self::Income => "Income",
            Self::Expense => "Expense",
        }
    }

    pub fn from_label(label: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|root| root.label().eq_ignore_ascii_case(label.trim()))
    }

    /// Asset, Liability and Equity accounts are on the balance sheet; the rest in
    /// profit and loss.
    pub fn is_balance_sheet(self) -> bool {
        matches!(self, Self::Asset | Self::Liability | Self::Equity)
    }
}

impl fmt::Display for RootType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// Frappe Books' account types, which tell the app what an account is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AccountType {
    #[serde(rename = "Accumulated Depreciation")]
    AccumulatedDepreciation,
    Bank,
    Cash,
    Chargeable,
    #[serde(rename = "Cost of Goods Sold")]
    CostOfGoodsSold,
    Depreciation,
    Equity,
    #[serde(rename = "Expense Account")]
    ExpenseAccount,
    #[serde(rename = "Expenses Included In Valuation")]
    ExpensesIncludedInValuation,
    #[serde(rename = "Fixed Asset")]
    FixedAsset,
    #[serde(rename = "Income Account")]
    IncomeAccount,
    Payable,
    Receivable,
    #[serde(rename = "Round Off")]
    RoundOff,
    Stock,
    #[serde(rename = "Stock Adjustment")]
    StockAdjustment,
    #[serde(rename = "Stock Received But Not Billed")]
    StockReceivedButNotBilled,
    Tax,
    Temporary,
}

impl AccountType {
    pub const ALL: [Self; 19] = [
        Self::AccumulatedDepreciation,
        Self::Bank,
        Self::Cash,
        Self::Chargeable,
        Self::CostOfGoodsSold,
        Self::Depreciation,
        Self::Equity,
        Self::ExpenseAccount,
        Self::ExpensesIncludedInValuation,
        Self::FixedAsset,
        Self::IncomeAccount,
        Self::Payable,
        Self::Receivable,
        Self::RoundOff,
        Self::Stock,
        Self::StockAdjustment,
        Self::StockReceivedButNotBilled,
        Self::Tax,
        Self::Temporary,
    ];

    /// The label Frappe Books stores and shows, e.g. `Cost of Goods Sold`.
    pub fn label(self) -> &'static str {
        match self {
            Self::AccumulatedDepreciation => "Accumulated Depreciation",
            Self::Bank => "Bank",
            Self::Cash => "Cash",
            Self::Chargeable => "Chargeable",
            Self::CostOfGoodsSold => "Cost of Goods Sold",
            Self::Depreciation => "Depreciation",
            Self::Equity => "Equity",
            Self::ExpenseAccount => "Expense Account",
            Self::ExpensesIncludedInValuation => "Expenses Included In Valuation",
            Self::FixedAsset => "Fixed Asset",
            Self::IncomeAccount => "Income Account",
            Self::Payable => "Payable",
            Self::Receivable => "Receivable",
            Self::RoundOff => "Round Off",
            Self::Stock => "Stock",
            Self::StockAdjustment => "Stock Adjustment",
            Self::StockReceivedButNotBilled => "Stock Received But Not Billed",
            Self::Tax => "Tax",
            Self::Temporary => "Temporary",
        }
    }

    pub fn from_label(label: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|kind| kind.label() == label.trim())
    }

    /// Money, parties, stock and assets are on the balance sheet.
    fn needs_balance_sheet(self) -> bool {
        matches!(
            self,
            Self::AccumulatedDepreciation
                | Self::Bank
                | Self::Cash
                | Self::Equity
                | Self::FixedAsset
                | Self::Payable
                | Self::Receivable
                | Self::Stock
                | Self::StockReceivedButNotBilled
                | Self::Temporary
        )
    }

    /// Sales and costs are in profit and loss.
    fn needs_profit_and_loss(self) -> bool {
        matches!(
            self,
            Self::CostOfGoodsSold | Self::ExpenseAccount | Self::IncomeAccount
        )
    }
}

impl fmt::Display for AccountType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccountError {
    /// "Account Type {0} needs a root type of Asset, Liability or Equity."
    NeedsBalanceSheet(AccountType),
    /// "Account Type {0} needs a root type of Income or Expense."
    NeedsProfitAndLoss(AccountType),
    /// An account type, once set, cannot change.
    TypeChanged,
    /// A root type, once set, cannot change.
    RootChanged,
    UnknownChart(String),
}

impl fmt::Display for AccountError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NeedsBalanceSheet(kind) => {
                write!(
                    f,
                    "Account Type {kind} needs a root type of Asset, Liability or Equity."
                )
            }
            Self::NeedsProfitAndLoss(kind) => {
                write!(
                    f,
                    "Account Type {kind} needs a root type of Income or Expense."
                )
            }
            Self::TypeChanged => f.write_str("An account type cannot be changed once it is set."),
            Self::RootChanged => f.write_str("A root type cannot be changed once it is set."),
            Self::UnknownChart(name) => write!(f, "Unknown chart of accounts: {name}"),
        }
    }
}

impl std::error::Error for AccountError {}

/// Checks that an account type suits the root type it falls under.
pub fn check_type_for_root(account_type: AccountType, root: RootType) -> Result<(), AccountError> {
    if account_type.needs_balance_sheet() && !root.is_balance_sheet() {
        return Err(AccountError::NeedsBalanceSheet(account_type));
    }
    if account_type.needs_profit_and_loss() && root.is_balance_sheet() {
        return Err(AccountError::NeedsProfitAndLoss(account_type));
    }
    Ok(())
}

/// An account type may be filled in later, but never changed once set.
pub fn check_type_change(
    saved: Option<AccountType>,
    new: Option<AccountType>,
) -> Result<(), AccountError> {
    match (saved, new) {
        (Some(saved), new) if Some(saved) != new => Err(AccountError::TypeChanged),
        _ => Ok(()),
    }
}

/// One account of a chart template, in tree order (parents before children).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ChartAccount {
    pub name: String,
    pub parent: Option<String>,
    pub root_type: RootType,
    pub account_type: Option<AccountType>,
    pub is_group: bool,
}

/// A chart template's name and the ledgers it names for the default settings accounts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ChartInfo {
    pub name: String,
    pub country_code: String,
    pub receivable: Option<String>,
    pub payable: Option<String>,
}

pub const STANDARD_CHART: &str = "Standard Chart of Accounts";
pub const TALLY_CHART: &str = "Tally (India) - Reserved Groups";

const COUNTRY_CHARTS: &[&str] = &[
    include_str!("../charts/ae.json"),
    include_str!("../charts/ca.json"),
    include_str!("../charts/ch.json"),
    include_str!("../charts/fr.json"),
    include_str!("../charts/gt.json"),
    include_str!("../charts/hu.json"),
    include_str!("../charts/id.json"),
    include_str!("../charts/in.json"),
    include_str!("../charts/mx.json"),
    include_str!("../charts/ni.json"),
    include_str!("../charts/nl.json"),
    include_str!("../charts/sg.json"),
];
const STANDARD_JSON: &str = include_str!("../charts/standardCOA.json");

/// Keys of a chart node that describe it rather than name a child account.
const META_KEYS: &[&str] = &["accountType", "accountNumber", "rootType", "isGroup"];

/// The charts a new book can start from: Tally's groups, the standard chart, then
/// Frappe Books' country charts.
pub fn charts() -> Vec<ChartInfo> {
    let mut list = vec![
        ChartInfo {
            name: TALLY_CHART.into(),
            country_code: "in".into(),
            receivable: None,
            payable: None,
        },
        ChartInfo {
            name: STANDARD_CHART.into(),
            country_code: String::new(),
            receivable: None,
            payable: None,
        },
    ];
    for text in COUNTRY_CHARTS {
        let chart: Value = serde_json::from_str(text).expect("bundled charts are valid JSON");
        let defaults = &chart["defaultAccounts"];
        list.push(ChartInfo {
            name: chart["name"].as_str().unwrap_or_default().to_owned(),
            country_code: chart["countryCode"].as_str().unwrap_or_default().to_owned(),
            receivable: defaults["receivable"].as_str().map(str::to_owned),
            payable: defaults["payable"].as_str().map(str::to_owned),
        });
    }
    list
}

/// The accounts of a chart template, parents before children.
pub fn load_chart(name: &str) -> Result<Vec<ChartAccount>, AccountError> {
    if name == TALLY_CHART {
        return Ok(tally_chart());
    }
    if name == STANDARD_CHART {
        let tree: Value = serde_json::from_str(STANDARD_JSON).expect("bundled chart is valid JSON");
        return Ok(flatten(&tree, None, None, None));
    }
    for text in COUNTRY_CHARTS {
        let chart: Value = serde_json::from_str(text).expect("bundled charts are valid JSON");
        if chart["name"] == name {
            return Ok(flatten(&chart["tree"], None, None, None));
        }
    }
    Err(AccountError::UnknownChart(name.to_owned()))
}

/// Children without an account type take their parent's, as Books Account does on save.
fn flatten(
    tree: &Value,
    parent: Option<&str>,
    root_type: Option<RootType>,
    account_type: Option<AccountType>,
) -> Vec<ChartAccount> {
    let mut accounts = Vec::new();
    let Some(nodes) = tree.as_object() else {
        return accounts;
    };
    for (label, node) in nodes {
        if META_KEYS.contains(&label.as_str()) || !node.is_object() {
            continue;
        }
        let name = match node.get("accountNumber").and_then(|n| {
            n.as_str()
                .map(str::to_owned)
                .or_else(|| n.as_i64().map(|i| i.to_string()))
        }) {
            Some(number) if !number.is_empty() => format!("{} - {number}", label.trim()),
            _ => label.trim().to_owned(),
        };
        let root = match parent {
            None => node["rootType"].as_str().and_then(RootType::from_label),
            Some(_) => root_type,
        };
        let Some(root) = root else { continue };
        let kind = node["accountType"]
            .as_str()
            .and_then(AccountType::from_label)
            .or(account_type);
        let has_children = node.as_object().is_some_and(|o| {
            o.iter()
                .any(|(k, v)| !META_KEYS.contains(&k.as_str()) && v.is_object())
        });
        accounts.push(ChartAccount {
            name: name.clone(),
            parent: parent.map(str::to_owned),
            root_type: root,
            account_type: kind,
            is_group: has_children || node["isGroup"].as_bool().unwrap_or(false),
        });
        accounts.extend(flatten(node, Some(&name), Some(root), kind));
    }
    accounts
}

/// Tally's 28 reserved groups: (name, parent, root type, account type).
const TALLY_GROUPS: &[(&str, Option<&str>, RootType, Option<AccountType>)] = &[
    ("Branch / Divisions", None, RootType::Liability, None),
    ("Capital Account", None, RootType::Equity, None),
    ("Current Assets", None, RootType::Asset, None),
    ("Current Liabilities", None, RootType::Liability, None),
    ("Direct Expenses", None, RootType::Expense, None),
    ("Direct Incomes", None, RootType::Income, None),
    (
        "Fixed Assets",
        None,
        RootType::Asset,
        Some(AccountType::FixedAsset),
    ),
    ("Indirect Expenses", None, RootType::Expense, None),
    ("Indirect Incomes", None, RootType::Income, None),
    ("Investments", None, RootType::Asset, None),
    ("Loans (Liability)", None, RootType::Liability, None),
    ("Misc. Expenses (ASSET)", None, RootType::Asset, None),
    (
        "Purchase Accounts",
        None,
        RootType::Expense,
        Some(AccountType::ExpenseAccount),
    ),
    (
        "Sales Accounts",
        None,
        RootType::Income,
        Some(AccountType::IncomeAccount),
    ),
    ("Suspense A/c", None, RootType::Liability, None),
    (
        "Bank Accounts",
        Some("Current Assets"),
        RootType::Asset,
        Some(AccountType::Bank),
    ),
    (
        "Bank OD A/c",
        Some("Loans (Liability)"),
        RootType::Liability,
        None,
    ),
    (
        "Cash-in-Hand",
        Some("Current Assets"),
        RootType::Asset,
        Some(AccountType::Cash),
    ),
    (
        "Deposits (Asset)",
        Some("Current Assets"),
        RootType::Asset,
        None,
    ),
    (
        "Duties & Taxes",
        Some("Current Liabilities"),
        RootType::Liability,
        Some(AccountType::Tax),
    ),
    (
        "Loans & Advances (Asset)",
        Some("Current Assets"),
        RootType::Asset,
        None,
    ),
    (
        "Provisions",
        Some("Current Liabilities"),
        RootType::Liability,
        None,
    ),
    (
        "Reserves & Surplus",
        Some("Capital Account"),
        RootType::Equity,
        None,
    ),
    (
        "Secured Loans",
        Some("Loans (Liability)"),
        RootType::Liability,
        None,
    ),
    (
        "Stock-in-Hand",
        Some("Current Assets"),
        RootType::Asset,
        Some(AccountType::Stock),
    ),
    (
        "Sundry Creditors",
        Some("Current Liabilities"),
        RootType::Liability,
        Some(AccountType::Payable),
    ),
    (
        "Sundry Debtors",
        Some("Current Assets"),
        RootType::Asset,
        Some(AccountType::Receivable),
    ),
    (
        "Unsecured Loans",
        Some("Loans (Liability)"),
        RootType::Liability,
        None,
    ),
];

/// Ledgers the Tally chart adds, so the settings defaults have accounts to point at.
const TALLY_LEDGERS: &[(&str, &str, Option<AccountType>)] = &[
    ("Cash", "Cash-in-Hand", Some(AccountType::Cash)),
    (
        "Rounded Off",
        "Indirect Expenses",
        Some(AccountType::RoundOff),
    ),
    ("Discounts", "Indirect Incomes", None),
    ("Write Off", "Indirect Expenses", None),
];

fn tally_chart() -> Vec<ChartAccount> {
    let root_of = |name: &str| TALLY_GROUPS.iter().find(|g| g.0 == name).map(|g| g.2);
    let mut accounts: Vec<ChartAccount> = TALLY_GROUPS
        .iter()
        .map(|(name, parent, root, kind)| ChartAccount {
            name: (*name).into(),
            parent: parent.map(str::to_owned),
            root_type: *root,
            account_type: *kind,
            is_group: true,
        })
        .collect();
    accounts.extend(
        TALLY_LEDGERS
            .iter()
            .map(|(name, group, kind)| ChartAccount {
                name: (*name).into(),
                parent: Some((*group).into()),
                root_type: root_of(group).expect("Tally ledgers sit in Tally groups"),
                account_type: *kind,
                is_group: false,
            }),
    );
    accounts
}

/// The root type of a group with one of Tally's reserved names, for books imported
/// from Tally or made before groups had root types. Tally's own spellings vary
/// ("Loans (Liability)", "Loans(Liability)"), so names are compared without spaces.
pub fn tally_root_type(group_name: &str) -> Option<RootType> {
    let key = |s: &str| {
        s.chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>()
            .to_lowercase()
    };
    let wanted = key(group_name);
    TALLY_GROUPS
        .iter()
        .find(|(name, ..)| key(name) == wanted)
        .map(|(_, _, root, _)| *root)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Ported from books_account.py validate_account_type_root.
    #[test]
    fn account_types_need_a_matching_root() {
        assert!(check_type_for_root(AccountType::Bank, RootType::Asset).is_ok());
        assert_eq!(
            check_type_for_root(AccountType::Bank, RootType::Expense),
            Err(AccountError::NeedsBalanceSheet(AccountType::Bank))
        );
        assert_eq!(
            check_type_for_root(AccountType::IncomeAccount, RootType::Liability),
            Err(AccountError::NeedsProfitAndLoss(AccountType::IncomeAccount))
        );
        // Types in neither list fit any root.
        assert!(check_type_for_root(AccountType::Tax, RootType::Liability).is_ok());
        assert!(check_type_for_root(AccountType::RoundOff, RootType::Expense).is_ok());
    }

    // Ported from books_account.py validate_account_type_change.
    #[test]
    fn an_account_type_is_set_once() {
        assert!(check_type_change(None, Some(AccountType::Cash)).is_ok());
        assert!(check_type_change(Some(AccountType::Cash), Some(AccountType::Cash)).is_ok());
        assert_eq!(
            check_type_change(Some(AccountType::Cash), Some(AccountType::Bank)),
            Err(AccountError::TypeChanged)
        );
        assert_eq!(
            check_type_change(Some(AccountType::Cash), None),
            Err(AccountError::TypeChanged)
        );
    }

    #[test]
    fn labels_round_trip() {
        for kind in AccountType::ALL {
            assert_eq!(AccountType::from_label(kind.label()), Some(kind));
            assert_eq!(serde_json::to_value(kind).unwrap(), kind.label());
        }
        for root in RootType::ALL {
            assert_eq!(RootType::from_label(root.label()), Some(root));
        }
    }

    #[test]
    fn every_bundled_chart_loads_with_valid_types() {
        let charts = charts();
        assert_eq!(charts.len(), 14);
        for info in &charts {
            let accounts = load_chart(&info.name).unwrap();
            assert!(!accounts.is_empty(), "{}", info.name);
            for account in &accounts {
                if let Some(parent) = &account.parent {
                    let parent = accounts.iter().find(|a| &a.name == parent).unwrap();
                    assert!(parent.is_group, "{} in {}", account.name, info.name);
                    assert_eq!(parent.root_type, account.root_type);
                }
            }
        }
        assert!(load_chart("Nowhere").is_err());
    }

    // Ported from coa.py _flatten: inherited types, groups by children or isGroup.
    #[test]
    fn india_chart_matches_frappe_books() {
        let chart = load_chart("India - Chart of Accounts").unwrap();
        let find = |name: &str| chart.iter().find(|a| a.name == name).unwrap();
        let debtors = find("Debtors");
        assert_eq!(
            (
                debtors.parent.as_deref(),
                debtors.root_type,
                debtors.account_type,
                debtors.is_group
            ),
            (
                Some("Accounts Receivable"),
                RootType::Asset,
                Some(AccountType::Receivable),
                false
            )
        );
        assert!(find("Bank Accounts").is_group, "isGroup without children");
        let stock = find("Stock In Hand");
        assert_eq!(stock.account_type, Some(AccountType::Stock));
        assert_eq!(find("CGST").root_type, RootType::Liability);
        assert_eq!(find("Sales").account_type, Some(AccountType::IncomeAccount));
    }

    #[test]
    fn numbered_charts_put_the_number_in_the_name() {
        let chart = load_chart("Indonesia - Chart of Accounts").unwrap();
        assert!(
            chart.iter().any(|a| a.name.ends_with(" - 1121.000")),
            "{:?}",
            &chart[..3]
        );
    }

    #[test]
    fn tally_groups_are_complete_and_recognised_by_name() {
        let chart = load_chart(TALLY_CHART).unwrap();
        assert_eq!(chart.iter().filter(|a| a.is_group).count(), 28);
        assert_eq!(
            chart
                .iter()
                .filter(|a| a.is_group && a.parent.is_none())
                .count(),
            15
        );
        assert_eq!(tally_root_type("Sundry Debtors"), Some(RootType::Asset));
        assert_eq!(
            tally_root_type("loans(liability)"),
            Some(RootType::Liability)
        );
        assert_eq!(tally_root_type("Sales Accounts"), Some(RootType::Income));
        assert_eq!(tally_root_type("Office Expenses"), None);
        for account in &chart {
            if let Some(kind) = account.account_type {
                assert!(
                    check_type_for_root(kind, account.root_type).is_ok(),
                    "{}",
                    account.name
                );
            }
        }
    }
}
