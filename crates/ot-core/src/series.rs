// Derived from Frappe Books (AGPL-3.0) frappe_books/series.py and
// frappe_books/frappe_books/doctype/books_number_series @953e9b4, modified:
// the counter is passed in rather than kept in Frappe's tabSeries.

//! Number series that name documents, e.g. `SINV-1001`.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Characters a series prefix cannot contain, because it shows in URLs.
pub const INVALID_PREFIX_CHARACTERS: &[char] = &['/', '=', '?', '&', '%'];
/// A new Number Series counts from here, with this many digits.
pub const DEFAULT_START: u64 = 1001;
pub const DEFAULT_PAD_ZEROS: u8 = 4;

/// The document types numbered by a series, with their standard prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SeriesType {
    JournalEntry,
    Payment,
    PurchaseInvoice,
    PricingRule,
    PurchaseReceipt,
    Shipment,
    SalesInvoice,
    StockMovement,
    SalesQuote,
}

impl SeriesType {
    pub const ALL: [Self; 9] = [
        Self::JournalEntry,
        Self::Payment,
        Self::PurchaseInvoice,
        Self::PricingRule,
        Self::PurchaseReceipt,
        Self::Shipment,
        Self::SalesInvoice,
        Self::StockMovement,
        Self::SalesQuote,
    ];

    /// The stable key used in storage and the API, e.g. `sales_invoice`.
    pub fn key(self) -> &'static str {
        match self {
            Self::JournalEntry => "journal_entry",
            Self::Payment => "payment",
            Self::PurchaseInvoice => "purchase_invoice",
            Self::PricingRule => "pricing_rule",
            Self::PurchaseReceipt => "purchase_receipt",
            Self::Shipment => "shipment",
            Self::SalesInvoice => "sales_invoice",
            Self::StockMovement => "stock_movement",
            Self::SalesQuote => "sales_quote",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.key() == key)
    }

    /// The document type's name for people, e.g. "Sales Invoice".
    pub fn label(self) -> &'static str {
        match self {
            Self::JournalEntry => "Journal Entry",
            Self::Payment => "Payment",
            Self::PurchaseInvoice => "Purchase Invoice",
            Self::PricingRule => "Pricing Rule",
            Self::PurchaseReceipt => "Purchase Receipt",
            Self::Shipment => "Shipment",
            Self::SalesInvoice => "Sales Invoice",
            Self::StockMovement => "Stock Movement",
            Self::SalesQuote => "Sales Quote",
        }
    }

    /// The prefix used when no series is chosen.
    pub fn standard_prefix(self) -> &'static str {
        match self {
            Self::JournalEntry => "JV-",
            Self::Payment => "PAY-",
            Self::PurchaseInvoice => "PINV-",
            Self::PricingRule => "PRLE-",
            Self::PurchaseReceipt => "PREC-",
            Self::Shipment => "SHPM-",
            Self::SalesInvoice => "SINV-",
            Self::StockMovement => "SMOV-",
            Self::SalesQuote => "SQUOT-",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SeriesError {
    InvalidPrefix(String),
    /// "Number series {0} is not for {1} documents."
    WrongType {
        series: String,
        for_type: SeriesType,
    },
    Exhausted {
        series: String,
    },
}

impl fmt::Display for SeriesError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPrefix(prefix) => write!(
                f,
                "The following characters cannot be used / , ? , & , = , % in a Number Series name: {prefix:?}."
            ),
            Self::WrongType { series, for_type } => {
                write!(
                    f,
                    "Number series {series} is not for {for_type:?} documents."
                )
            }
            Self::Exhausted { series } => write!(f, "Number series {series} has no numbers left."),
        }
    }
}

impl std::error::Error for SeriesError {}

/// A Books Number Series. Its name is the prefix.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NumberSeries {
    pub prefix: String,
    pub reference_type: SeriesType,
    pub start: u64,
    pub pad_zeros: u8,
    /// The last number used; `start - 1` before the first document.
    pub current: u64,
}

impl NumberSeries {
    pub fn new(prefix: &str, reference_type: SeriesType) -> Result<Self, SeriesError> {
        Self::with_start(prefix, reference_type, DEFAULT_START, DEFAULT_PAD_ZEROS)
    }

    pub fn with_start(
        prefix: &str,
        reference_type: SeriesType,
        start: u64,
        pad_zeros: u8,
    ) -> Result<Self, SeriesError> {
        validate_prefix(prefix)?;
        Ok(Self {
            prefix: prefix.to_owned(),
            reference_type,
            start,
            pad_zeros: pad_zeros.max(1),
            current: start.saturating_sub(1),
        })
    }

    /// The standard series for a document type, e.g. `SINV-` from 1001.
    pub fn standard(reference_type: SeriesType) -> Self {
        Self::new(reference_type.standard_prefix(), reference_type)
            .expect("standard prefixes are valid")
    }

    /// The name the next document would get, without taking it.
    pub fn peek(&self) -> Result<String, SeriesError> {
        let next = self
            .current
            .checked_add(1)
            .ok_or_else(|| SeriesError::Exhausted {
                series: self.prefix.clone(),
            })?;
        Ok(format_name(&self.prefix, next, self.pad_zeros))
    }

    /// Takes the next name for a document of `doctype`.
    pub fn next_name(&mut self, doctype: SeriesType) -> Result<String, SeriesError> {
        if doctype != self.reference_type {
            return Err(SeriesError::WrongType {
                series: self.prefix.clone(),
                for_type: doctype,
            });
        }
        let name = self.peek()?;
        self.current += 1;
        Ok(name)
    }

    /// Restarts counting from `start` unless the series already counted past it.
    pub fn restart_from(&mut self, start: u64) {
        self.current = self.current.max(start.saturating_sub(1));
    }
}

/// `prefix` followed by `number` padded to `digits`, as Frappe's `PREFIX.####`.
pub fn format_name(prefix: &str, number: u64, digits: u8) -> String {
    format!(
        "{prefix}{number:0width$}",
        width = usize::from(digits.max(1))
    )
}

/// A prefix shows in URLs and must not be empty or contain `/ = ? & %`.
pub fn validate_prefix(prefix: &str) -> Result<(), SeriesError> {
    if prefix.trim().is_empty()
        || prefix.contains(INVALID_PREFIX_CHARACTERS)
        || prefix.chars().any(char::is_control)
    {
        return Err(SeriesError::InvalidPrefix(prefix.to_owned()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_series_counts_from_1001_with_four_digits() {
        let mut series = NumberSeries::standard(SeriesType::SalesInvoice);
        assert_eq!(series.peek().unwrap(), "SINV-1001");
        assert_eq!(
            series.next_name(SeriesType::SalesInvoice).unwrap(),
            "SINV-1001"
        );
        assert_eq!(
            series.next_name(SeriesType::SalesInvoice).unwrap(),
            "SINV-1002"
        );
        assert_eq!(series.current, 1002);
    }

    #[test]
    fn padding_and_custom_start() {
        assert_eq!(
            NumberSeries::with_start("JV/", SeriesType::JournalEntry, 1, 4),
            Err(SeriesError::InvalidPrefix("JV/".into()))
        );
        let mut jv = NumberSeries::with_start("JV-2026-", SeriesType::JournalEntry, 7, 3).unwrap();
        assert_eq!(
            jv.next_name(SeriesType::JournalEntry).unwrap(),
            "JV-2026-007"
        );
        // A number wider than the padding is written in full.
        assert_eq!(format_name("X-", 12345, 4), "X-12345");
        let mut full = NumberSeries::with_start("Z-", SeriesType::Payment, 1, 1).unwrap();
        full.current = u64::MAX;
        assert!(matches!(full.peek(), Err(SeriesError::Exhausted { .. })));
    }

    // Ported from series.py validate_series_type: a series numbers only its own type.
    #[test]
    fn series_numbers_only_its_reference_type() {
        let mut series = NumberSeries::standard(SeriesType::Payment);
        assert_eq!(
            series.next_name(SeriesType::SalesInvoice),
            Err(SeriesError::WrongType {
                series: "PAY-".into(),
                for_type: SeriesType::SalesInvoice
            })
        );
        assert_eq!(series.current, 1000, "a refused name does not use a number");
    }

    // Ported from series.py start_series: count from start unless already past it.
    #[test]
    fn restart_never_goes_back() {
        let mut series = NumberSeries::standard(SeriesType::Shipment);
        series.restart_from(2001);
        assert_eq!(series.peek().unwrap(), "SHPM-2001");
        series.restart_from(1500);
        assert_eq!(series.peek().unwrap(), "SHPM-2001");
    }

    #[test]
    fn keys_round_trip_and_match_serde() {
        for kind in SeriesType::ALL {
            assert_eq!(SeriesType::from_key(kind.key()), Some(kind));
            assert_eq!(
                serde_json::to_value(kind).unwrap(),
                serde_json::json!(kind.key())
            );
        }
        assert_eq!(SeriesType::from_key("Sales Invoice"), None);
    }

    #[test]
    fn invalid_prefixes_are_refused() {
        for prefix in ["", "  ", "A/B", "A=B", "A?B", "A&B", "A%B", "A\nB"] {
            assert!(validate_prefix(prefix).is_err(), "{prefix:?}");
        }
        for kind in SeriesType::ALL {
            assert!(validate_prefix(kind.standard_prefix()).is_ok());
        }
    }
}
