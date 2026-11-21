// Derived from Frappe Books (AGPL-3.0) frappe_books/accounting/money.py and
// frappe_books/currency.py @953e9b4, modified: Rust types, minor-unit storage,
// and a built-in CLDR precision table in place of Babel.

//! Money in whole minor units of a currency, and decimal rounding to that unit.
//!
//! Calculations (rates, discounts, tax) run on [`Decimal`] at full precision and
//! become [`Money`] once, through [`Currency::round`], as Frappe Books rounds each
//! amount with `ROUND_HALF_UP` to the currency's smallest unit.

use std::{fmt, str::FromStr};

pub use rust_decimal::Decimal;
use rust_decimal::RoundingStrategy;
use serde::{Deserialize, Serialize};

/// Amounts must stay below 10^12 whole units. Frappe Books stores numbers as
/// `decimal(21,9)`, which has 12 digits before the point.
pub const NUMBER_LIMIT_UNITS: i64 = 1_000_000_000_000;

/// CLDR fraction digits for currencies that do not use two.
/// Everything not listed uses two, which is CLDR's default.
/// Currencies offered first in forms: those of the bundled charts and the most traded.
/// Any ISO 4217 code is accepted.
pub const COMMON_CURRENCIES: &[&str] = &[
    "INR", "USD", "EUR", "GBP", "AED", "SAR", "QAR", "KWD", "OMR", "BHD", "SGD", "HKD", "JPY",
    "CNY", "AUD", "NZD", "CAD", "CHF", "ZAR", "BDT", "NPR", "LKR", "IDR", "MYR", "THB", "MXN",
    "GTQ", "NIO", "HUF",
];

const PRECISIONS: &[(&str, u8)] = &[
    ("BHD", 3),
    ("BIF", 0),
    ("CLF", 4),
    ("CLP", 0),
    ("DJF", 0),
    ("GNF", 0),
    ("IQD", 0),
    ("ISK", 0),
    ("JOD", 3),
    ("JPY", 0),
    ("KMF", 0),
    ("KRW", 0),
    ("KWD", 3),
    ("LYD", 3),
    ("OMR", 3),
    ("PYG", 0),
    ("RWF", 0),
    ("TND", 3),
    ("UGX", 0),
    ("UYI", 0),
    ("UYW", 4),
    ("VND", 0),
    ("VUV", 0),
    ("XAF", 0),
    ("XOF", 0),
    ("XPF", 0),
];

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Currency {
    code: String,
    precision: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MoneyError {
    UnknownCurrency(String),
    /// Not a number, or more decimal places than the currency allows.
    Invalid(String),
    /// At or beyond [`NUMBER_LIMIT_UNITS`].
    TooLarge,
    /// Two amounts in different currencies were combined.
    CurrencyMismatch,
}

impl fmt::Display for MoneyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownCurrency(code) => write!(f, "{code} is not a currency code"),
            Self::Invalid(text) => write!(f, "{text} is not a valid amount"),
            Self::TooLarge => f.write_str("The amount is too large."),
            Self::CurrencyMismatch => {
                f.write_str("Amounts in different currencies cannot be combined.")
            }
        }
    }
}

impl std::error::Error for MoneyError {}

impl Currency {
    /// A currency by its ISO 4217 code, with its CLDR precision.
    pub fn new(code: &str) -> Result<Self, MoneyError> {
        let code = code.trim().to_ascii_uppercase();
        if code.len() != 3 || !code.bytes().all(|b| b.is_ascii_uppercase()) {
            return Err(MoneyError::UnknownCurrency(code));
        }
        let precision = PRECISIONS
            .iter()
            .find(|(c, _)| *c == code)
            .map_or(2, |(_, p)| *p);
        Ok(Self { code, precision })
    }

    /// A currency with an explicit precision, for currencies a book defines itself.
    pub fn with_precision(code: &str, precision: u8) -> Result<Self, MoneyError> {
        let mut currency = Self::new(code)?;
        if precision > 6 {
            return Err(MoneyError::Invalid(format!("precision {precision}")));
        }
        currency.precision = precision;
        Ok(currency)
    }

    pub fn inr() -> Self {
        Self {
            code: "INR".into(),
            precision: 2,
        }
    }

    pub fn code(&self) -> &str {
        &self.code
    }

    /// Fraction digits, e.g. 2 for INR, 0 for JPY, 3 for BHD.
    pub fn precision(&self) -> u8 {
        self.precision
    }

    /// The smallest unit as a decimal, e.g. 0.01.
    pub fn smallest_unit(&self) -> Decimal {
        Decimal::new(1, u32::from(self.precision))
    }

    /// Rounds half away from zero to the smallest unit, as Python's `ROUND_HALF_UP`.
    pub fn round(&self, value: Decimal) -> Result<Money, MoneyError> {
        let rounded = value.round_dp_with_strategy(
            u32::from(self.precision),
            RoundingStrategy::MidpointAwayFromZero,
        );
        let minor = rounded
            .checked_mul(Decimal::from(10_i64.pow(u32::from(self.precision))))
            .ok_or(MoneyError::TooLarge)?;
        let minor = i128::try_from(minor.trunc()).map_err(|_| MoneyError::TooLarge)?;
        Money::from_minor(minor, self.clone())
    }

    /// Parses an amount written with at most this currency's decimal places, e.g. "250.5".
    pub fn parse(&self, text: &str) -> Result<Money, MoneyError> {
        let value = Decimal::from_str(text.trim()).map_err(|_| MoneyError::Invalid(text.into()))?;
        if value.normalize().scale() > u32::from(self.precision) {
            return Err(MoneyError::Invalid(text.into()));
        }
        self.round(value)
    }
}

/// An amount in whole minor units (paise for INR) of one currency.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Money {
    minor: i128,
    currency: Currency,
}

impl Money {
    pub fn from_minor(minor: i128, currency: Currency) -> Result<Self, MoneyError> {
        let limit = i128::from(NUMBER_LIMIT_UNITS) * 10_i128.pow(u32::from(currency.precision));
        if minor.abs() >= limit {
            return Err(MoneyError::TooLarge);
        }
        Ok(Self { minor, currency })
    }

    pub fn zero(currency: Currency) -> Self {
        Self { minor: 0, currency }
    }

    pub fn minor(&self) -> i128 {
        self.minor
    }

    pub fn currency(&self) -> &Currency {
        &self.currency
    }

    pub fn is_zero(&self) -> bool {
        self.minor == 0
    }

    pub fn is_negative(&self) -> bool {
        self.minor < 0
    }

    pub fn abs(&self) -> Self {
        Self {
            minor: self.minor.abs(),
            currency: self.currency.clone(),
        }
    }

    pub fn neg(&self) -> Self {
        Self {
            minor: -self.minor,
            currency: self.currency.clone(),
        }
    }

    pub fn to_decimal(&self) -> Decimal {
        Decimal::from_i128_with_scale(self.minor, u32::from(self.currency.precision))
    }

    pub fn checked_add(&self, other: &Self) -> Result<Self, MoneyError> {
        if self.currency != other.currency {
            return Err(MoneyError::CurrencyMismatch);
        }
        Self::from_minor(
            self.minor
                .checked_add(other.minor)
                .ok_or(MoneyError::TooLarge)?,
            self.currency.clone(),
        )
    }

    pub fn checked_sub(&self, other: &Self) -> Result<Self, MoneyError> {
        self.checked_add(&other.neg())
    }
}

impl fmt::Display for Money {
    /// The plain amount with the currency's decimal places, e.g. `-1250.50`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_decimal())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dec(text: &str) -> Decimal {
        Decimal::from_str(text).unwrap()
    }

    // Ported from tests/test_currency.py::test_currency_precision_follows_cldr.
    #[test]
    fn currency_precision_follows_cldr() {
        for (code, precision) in [("JPY", 0), ("VUV", 0), ("USD", 2), ("BHD", 3), ("CLF", 4)] {
            assert_eq!(
                Currency::new(code).unwrap().precision(),
                precision,
                "{code}"
            );
        }
        assert_eq!(Currency::new("inr").unwrap(), Currency::inr());
        assert!(Currency::new("RUPEE").is_err());
    }

    #[test]
    fn rounding_is_half_up_away_from_zero() {
        let inr = Currency::inr();
        assert_eq!(inr.round(dec("2.345")).unwrap().minor(), 235);
        assert_eq!(inr.round(dec("-2.345")).unwrap().minor(), -235);
        assert_eq!(inr.round(dec("2.344")).unwrap().minor(), 234);
        let jpy = Currency::new("JPY").unwrap();
        assert_eq!(jpy.round(dec("1250.5")).unwrap().minor(), 1251);
        let bhd = Currency::new("BHD").unwrap();
        assert_eq!(bhd.round(dec("1.0005")).unwrap().to_string(), "1.001");
    }

    #[test]
    fn parsing_respects_currency_precision() {
        let inr = Currency::inr();
        assert_eq!(inr.parse("250").unwrap().minor(), 25000);
        assert_eq!(inr.parse("250.50").unwrap().to_string(), "250.50");
        assert!(inr.parse("1.234").is_err());
        assert!(inr.parse("abc").is_err());
        assert_eq!(
            Currency::new("JPY").unwrap().parse("1.0").unwrap().minor(),
            1
        );
        assert!(Currency::new("JPY").unwrap().parse("1.5").is_err());
    }

    #[test]
    fn amounts_stay_below_the_number_limit() {
        let inr = Currency::inr();
        assert!(inr.parse("999999999999.99").is_ok());
        assert_eq!(inr.parse("1000000000000"), Err(MoneyError::TooLarge));
        let big = inr.parse("999999999999.99").unwrap();
        assert_eq!(
            big.checked_add(&inr.parse("0.01").unwrap()),
            Err(MoneyError::TooLarge)
        );
    }

    #[test]
    fn currencies_do_not_mix() {
        let a = Currency::inr().parse("1").unwrap();
        let b = Currency::new("USD").unwrap().parse("1").unwrap();
        assert_eq!(a.checked_add(&b), Err(MoneyError::CurrencyMismatch));
        assert_eq!(a.checked_sub(&a).unwrap(), Money::zero(Currency::inr()));
    }
}
