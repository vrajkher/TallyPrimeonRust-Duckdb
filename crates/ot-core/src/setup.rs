// Derived from Frappe Books (AGPL-3.0)
// frappe_books/frappe_books/doctype/books_setup_wizard/books_setup_wizard.py,
// frappe_books/settings.py (validate_fiscal_year) and frappe_books/data/country_info.json
// @953e9b4, modified: pure functions over dates, and a currency table for the
// countries OpenTally has charts for (Frappe takes currencies from its framework).

//! What the setup wizard suggests for a country, and the fiscal-year rule.

use std::{collections::BTreeMap, sync::OnceLock};

use chrono::{Datelike, Months, NaiveDate};
use serde::{Deserialize, Serialize};

use crate::accounts::{STANDARD_CHART, TALLY_CHART, charts};

#[derive(Debug, Clone, Deserialize)]
struct CountryInfo {
    fiscal_year_start: Option<String>,
    locale: Option<String>,
}

fn country_info() -> &'static BTreeMap<String, CountryInfo> {
    static INFO: OnceLock<BTreeMap<String, CountryInfo>> = OnceLock::new();
    INFO.get_or_init(|| {
        serde_json::from_str(include_str!("../charts/country_info.json"))
            .expect("bundled country data is valid JSON")
    })
}

/// The currency of countries OpenTally has a chart for, and a few more.
const CURRENCIES: &[(&str, &str)] = &[
    ("Bangladesh", "BDT"),
    ("Canada", "CAD"),
    ("France", "EUR"),
    ("Guatemala", "GTQ"),
    ("Hungary", "HUF"),
    ("India", "INR"),
    ("Indonesia", "IDR"),
    ("Mexico", "MXN"),
    ("Nepal", "NPR"),
    ("Netherlands", "EUR"),
    ("Nicaragua", "NIO"),
    ("Singapore", "SGD"),
    ("Sri Lanka", "LKR"),
    ("Switzerland", "CHF"),
    ("United Arab Emirates", "AED"),
    ("United Kingdom", "GBP"),
    ("United States", "USD"),
];

/// The countries the wizard offers, by Frappe's country names.
pub fn countries() -> Vec<String> {
    country_info().keys().cloned().collect()
}

/// What the wizard fills in for a country, as Frappe Books' `set_country_defaults`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CountryDefaults {
    pub country: String,
    pub currency: Option<String>,
    pub chart: String,
    pub fiscal_year_start: String,
    pub fiscal_year_end: String,
    pub locale: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SetupError {
    UnknownCountry(String),
    /// "Fiscal Year End Date must be after Fiscal Year Start Date."
    EndBeforeStart,
    /// "A fiscal year cannot be longer than one year."
    LongerThanAYear,
    InvalidDate(String),
}

impl std::fmt::Display for SetupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownCountry(name) => write!(f, "{name} is not a country OpenTally knows."),
            Self::EndBeforeStart => {
                f.write_str("Fiscal Year End Date must be after Fiscal Year Start Date.")
            }
            Self::LongerThanAYear => f.write_str("A fiscal year cannot be longer than one year."),
            Self::InvalidDate(text) => write!(f, "{text} is not a date in YYYY-MM-DD form."),
        }
    }
}

impl std::error::Error for SetupError {}

/// Today's date in UTC, from the system clock.
pub fn today_utc() -> NaiveDate {
    let days = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() / 86_400);
    NaiveDate::from_ymd_opt(1970, 1, 1)
        .and_then(|epoch| epoch.checked_add_days(chrono::Days::new(days)))
        .unwrap_or_default()
}

pub fn parse_date(text: &str) -> Result<NaiveDate, SetupError> {
    NaiveDate::parse_from_str(text.trim(), "%Y-%m-%d")
        .map_err(|_| SetupError::InvalidDate(text.into()))
}

/// The last day of a fiscal year that starts on `start`.
pub fn fiscal_year_end(start: NaiveDate) -> NaiveDate {
    start
        .checked_add_months(Months::new(12))
        .and_then(|d| d.pred_opt())
        .unwrap_or(start)
}

/// A fiscal year ends after it starts, at most a year later, as ERPNext's Fiscal Year does.
pub fn validate_fiscal_year(start: NaiveDate, end: NaiveDate) -> Result<(), SetupError> {
    if end <= start {
        return Err(SetupError::EndBeforeStart);
    }
    if end > fiscal_year_end(start) {
        return Err(SetupError::LongerThanAYear);
    }
    Ok(())
}

/// The chart the wizard suggests: Tally's groups for India, where OpenTally's users come
/// from Tally; else the country's Frappe Books chart; else the standard chart.
/// Charts are matched by the region code of the country's locale (`ar-AE` gives `ae`),
/// since chart names do not always spell the country as the country list does.
fn suggested_chart(country: &str, locale: Option<&str>) -> String {
    if country == "India" {
        return TALLY_CHART.to_owned();
    }
    let region = locale
        .and_then(|l| l.rsplit('-').next())
        .map(str::to_lowercase)
        .unwrap_or_default();
    charts()
        .into_iter()
        .find(|chart| !region.is_empty() && chart.country_code == region)
        .map_or_else(|| STANDARD_CHART.to_owned(), |chart| chart.name)
}

/// The wizard's suggestions for `country`, with the fiscal year that contains `today`.
/// Countries without a fiscal year in the data use the calendar year.
pub fn country_defaults(country: &str, today: NaiveDate) -> Result<CountryDefaults, SetupError> {
    let info = country_info()
        .get(country)
        .ok_or_else(|| SetupError::UnknownCountry(country.to_owned()))?;
    let (month, day) = info
        .fiscal_year_start
        .as_deref()
        .and_then(|md| md.split_once('-'))
        .and_then(|(m, d)| Some((m.parse::<u32>().ok()?, d.parse::<u32>().ok()?)))
        .unwrap_or((1, 1));
    let this_year = NaiveDate::from_ymd_opt(today.year(), month, day).unwrap_or(today);
    let start = if today >= this_year {
        this_year
    } else {
        NaiveDate::from_ymd_opt(today.year() - 1, month, day).unwrap_or(this_year)
    };
    Ok(CountryDefaults {
        country: country.to_owned(),
        currency: CURRENCIES
            .iter()
            .find(|(c, _)| *c == country)
            .map(|(_, code)| (*code).to_owned()),
        chart: suggested_chart(country, info.locale.as_deref()),
        fiscal_year_start: start.to_string(),
        fiscal_year_end: fiscal_year_end(start).to_string(),
        locale: info.locale.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(text: &str) -> NaiveDate {
        parse_date(text).unwrap()
    }

    #[test]
    fn india_suggests_rupees_tally_groups_and_an_april_year() {
        let before_april = country_defaults("India", date("2026-02-10")).unwrap();
        assert_eq!(before_april.currency.as_deref(), Some("INR"));
        assert_eq!(before_april.chart, TALLY_CHART);
        assert_eq!(
            (
                before_april.fiscal_year_start.as_str(),
                before_april.fiscal_year_end.as_str()
            ),
            ("2025-04-01", "2026-03-31")
        );
        let after = country_defaults("India", date("2026-11-20")).unwrap();
        assert_eq!(after.fiscal_year_start, "2026-04-01");
    }

    #[test]
    fn other_countries_get_their_chart_or_the_standard_one() {
        let uae = country_defaults("United Arab Emirates", date("2026-11-20")).unwrap();
        assert_eq!(uae.chart, "U.A.E - Chart of Accounts");
        assert_eq!(
            country_defaults("Netherlands", date("2026-01-01"))
                .unwrap()
                .chart,
            "Netherlands - Grootboekschema"
        );
        assert_eq!(
            (uae.fiscal_year_start.as_str(), uae.fiscal_year_end.as_str()),
            ("2026-01-01", "2026-12-31")
        );
        let bangladesh = country_defaults("Bangladesh", date("2026-03-01")).unwrap();
        assert_eq!(bangladesh.chart, STANDARD_CHART);
        assert_eq!(bangladesh.fiscal_year_start, "2025-07-01");
        assert!(country_defaults("Atlantis", date("2026-01-01")).is_err());
        assert!(countries().len() > 150);
    }

    // Ported from settings.py validate_fiscal_year.
    #[test]
    fn a_fiscal_year_is_at_most_one_year() {
        assert!(validate_fiscal_year(date("2025-04-01"), date("2026-03-31")).is_ok());
        assert_eq!(
            validate_fiscal_year(date("2025-04-01"), date("2025-04-01")),
            Err(SetupError::EndBeforeStart)
        );
        assert_eq!(
            validate_fiscal_year(date("2025-04-01"), date("2026-04-01")),
            Err(SetupError::LongerThanAYear)
        );
        assert_eq!(fiscal_year_end(date("2024-03-01")), date("2025-02-28"));
        assert!(parse_date("April").is_err());
    }
}
