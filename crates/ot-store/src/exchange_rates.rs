//! Exchange rates (roadmap step 1.8). Frappe Books fetches a rate from a web service
//! for each invoice; OpenTally keeps the rates you enter, so foreign-currency
//! documents work offline and the rate a document used can be shown later.

use std::str::FromStr;

use duckdb::{OptionalExt, params};
use ot_core::money::{Currency, Decimal};
use serde::{Deserialize, Serialize};

use crate::store::{Store, StoreError, validated_date};

/// One unit of `from_currency` is `rate` units of `to_currency` on `date`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExchangeRate {
    pub from_currency: String,
    pub to_currency: String,
    /// YYYY-MM-DD.
    pub date: String,
    /// A decimal number, e.g. "83.25".
    pub rate: String,
}

/// `GET /api/v1/exchange-rates` filters.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct RateFilter {
    #[serde(default)]
    pub from: Option<String>,
    #[serde(default)]
    pub to: Option<String>,
}

/// `GET /api/v1/exchange-rates/lookup`: the rate to use on a date.
#[derive(Debug, Clone, Deserialize)]
pub struct RateQuery {
    pub from: String,
    pub to: String,
    pub date: String,
}

/// The rate found for a lookup, and the entry it came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RateFound {
    pub rate: String,
    /// The date of the rate used: the latest on or before the date asked for.
    pub date: String,
    /// True when only the opposite pair was entered and the rate is its inverse.
    pub inverted: bool,
}

fn invalid(text: impl Into<String>) -> StoreError {
    StoreError::Invalid(text.into())
}

fn code(text: &str) -> Result<String, StoreError> {
    Currency::new(text)
        .map(|c| c.code().to_owned())
        .map_err(|_| invalid(format!("{} is not a currency code, e.g. USD.", text.trim())))
}

fn pair(from: &str, to: &str) -> Result<(String, String), StoreError> {
    let (from, to) = (code(from)?, code(to)?);
    if from == to {
        return Err(invalid("Choose two different currencies."));
    }
    Ok((from, to))
}

fn parse_rate(text: &str) -> Result<Decimal, StoreError> {
    Decimal::from_str(text.trim())
        .ok()
        .filter(|r| {
            *r > Decimal::ZERO
                && r.normalize().scale() <= 9
                && *r < Decimal::from(1_000_000_000_i64)
        })
        .ok_or_else(|| {
            invalid(format!(
                "{} is not an exchange rate. Enter a number greater than zero.",
                text.trim()
            ))
        })
}

fn show(text: String) -> String {
    Decimal::from_str(&text)
        .map(|d| d.normalize().to_string())
        .unwrap_or(text)
}

impl Store {
    pub fn exchange_rates(&self, filter: &RateFilter) -> Result<Vec<ExchangeRate>, StoreError> {
        let from = filter
            .from
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .map(code)
            .transpose()?;
        let to = filter
            .to
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .map(code)
            .transpose()?;
        let mut statement = self.connection.prepare(
            "SELECT from_currency, to_currency, CAST(rate_date AS VARCHAR), CAST(rate AS VARCHAR)
             FROM exchange_rates
             WHERE (CAST(? AS VARCHAR) IS NULL OR from_currency = ?)
               AND (CAST(? AS VARCHAR) IS NULL OR to_currency = ?)
             ORDER BY rate_date DESC, from_currency, to_currency",
        )?;
        let rows = statement
            .query_map(params![from, from, to, to], |row| {
                Ok(ExchangeRate {
                    from_currency: row.get(0)?,
                    to_currency: row.get(1)?,
                    date: row.get(2)?,
                    rate: show(row.get(3)?),
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Records the rate for a pair on a date, replacing one entered for that day.
    pub fn set_exchange_rate(&mut self, input: ExchangeRate) -> Result<ExchangeRate, StoreError> {
        let (from, to) = pair(&input.from_currency, &input.to_currency)?;
        let date = validated_date(input.date.trim())?.to_owned();
        let rate = parse_rate(&input.rate)?;
        let tx = self.connection.transaction()?;
        tx.execute(
            "DELETE FROM exchange_rates WHERE from_currency = ? AND to_currency = ? AND rate_date = CAST(? AS DATE)",
            params![from, to, date],
        )?;
        tx.execute(
            "INSERT INTO exchange_rates (from_currency, to_currency, rate_date, rate)
             VALUES (?, ?, CAST(? AS DATE), CAST(? AS DECIMAL(18, 9)))",
            params![from, to, date, rate.to_string()],
        )?;
        tx.commit()?;
        Ok(ExchangeRate {
            from_currency: from,
            to_currency: to,
            date,
            rate: rate.normalize().to_string(),
        })
    }

    pub fn delete_exchange_rate(
        &mut self,
        from: &str,
        to: &str,
        date: &str,
    ) -> Result<(), StoreError> {
        let (from, to) = pair(from, to)?;
        let date = validated_date(date.trim())?;
        let deleted = self.connection.execute(
            "DELETE FROM exchange_rates WHERE from_currency = ? AND to_currency = ? AND rate_date = CAST(? AS DATE)",
            params![from, to, date],
        )?;
        if deleted == 0 {
            return Err(StoreError::Missing(format!(
                "No {from} to {to} rate is recorded for {date}."
            )));
        }
        Ok(())
    }

    /// The rate to use on a date: the latest entered on or before it for the pair,
    /// else the inverse of the opposite pair's. `None` when neither has one.
    pub fn exchange_rate_on(&self, query: &RateQuery) -> Result<Option<RateFound>, StoreError> {
        let (from, to) = pair(&query.from, &query.to)?;
        let date = validated_date(query.date.trim())?;
        let latest = |from: &str, to: &str| -> Result<Option<(String, String)>, StoreError> {
            Ok(self
                .connection
                .query_row(
                    "SELECT CAST(rate AS VARCHAR), CAST(rate_date AS VARCHAR) FROM exchange_rates
                     WHERE from_currency = ? AND to_currency = ? AND rate_date <= CAST(? AS DATE)
                     ORDER BY rate_date DESC LIMIT 1",
                    params![from, to, date],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?)
        };
        let direct = latest(&from, &to)?;
        let opposite = latest(&to, &from)?;
        // Prefer the newer entry; on the same day, the pair as asked.
        let found = match (direct, opposite) {
            (Some(d), Some(o)) if o.1 > d.1 => Some((o, true)),
            (Some(d), _) => Some((d, false)),
            (None, Some(o)) => Some((o, true)),
            (None, None) => None,
        };
        Ok(found.map(|((rate, date), inverted)| {
            let mut value = Decimal::from_str(&rate).unwrap_or(Decimal::ONE);
            if inverted {
                value = (Decimal::ONE / value).round_dp(9);
            }
            RateFound {
                rate: value.normalize().to_string(),
                date,
                inverted,
            }
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rate(from: &str, to: &str, date: &str, rate: &str) -> ExchangeRate {
        ExchangeRate {
            from_currency: from.into(),
            to_currency: to.into(),
            date: date.into(),
            rate: rate.into(),
        }
    }

    fn query(from: &str, to: &str, date: &str) -> RateQuery {
        RateQuery {
            from: from.into(),
            to: to.into(),
            date: date.into(),
        }
    }

    #[test]
    fn the_latest_rate_on_or_before_the_date_is_used() {
        let mut store = Store::open_in_memory().unwrap();
        store
            .set_exchange_rate(rate("usd", "INR", "2026-04-01", "83.25"))
            .unwrap();
        store
            .set_exchange_rate(rate("USD", "INR", "2026-05-01", "84.100"))
            .unwrap();
        let on = |date: &str| store.exchange_rate_on(&query("USD", "INR", date)).unwrap();
        assert_eq!(on("2026-03-31"), None);
        let april = on("2026-04-20").unwrap();
        assert_eq!(
            (april.rate.as_str(), april.date.as_str()),
            ("83.25", "2026-04-01")
        );
        assert_eq!(on("2026-06-01").unwrap().rate, "84.1");
        // The opposite pair is the inverse.
        let inverse = store
            .exchange_rate_on(&query("INR", "USD", "2026-04-20"))
            .unwrap()
            .unwrap();
        assert!(inverse.inverted);
        assert_eq!(inverse.rate, "0.012012012");
    }

    #[test]
    fn a_day_has_one_rate_per_pair_and_bad_input_is_refused() {
        let mut store = Store::open_in_memory().unwrap();
        store
            .set_exchange_rate(rate("EUR", "INR", "2026-04-01", "90"))
            .unwrap();
        store
            .set_exchange_rate(rate("EUR", "INR", "2026-04-01", "91.5"))
            .unwrap();
        let rows = store.exchange_rates(&RateFilter::default()).unwrap();
        assert_eq!(rows, [rate("EUR", "INR", "2026-04-01", "91.5")]);
        for bad in [
            rate("EUR", "EUR", "2026-04-01", "1"),
            rate("EURO", "INR", "2026-04-01", "1"),
            rate("EUR", "INR", "2026-02-30", "1"),
            rate("EUR", "INR", "2026-04-01", "0"),
            rate("EUR", "INR", "2026-04-01", "-2"),
            rate("EUR", "INR", "2026-04-01", "ninety"),
        ] {
            assert!(
                matches!(
                    store.set_exchange_rate(bad.clone()),
                    Err(StoreError::Invalid(_))
                ),
                "{bad:?}"
            );
        }
        store
            .delete_exchange_rate("eur", "inr", "2026-04-01")
            .unwrap();
        assert!(matches!(
            store.delete_exchange_rate("EUR", "INR", "2026-04-01"),
            Err(StoreError::Missing(_))
        ));
    }

    #[test]
    fn rates_are_filtered_by_currency() {
        let mut store = Store::open_in_memory().unwrap();
        for (from, to) in [("USD", "INR"), ("EUR", "INR"), ("USD", "AED")] {
            store
                .set_exchange_rate(rate(from, to, "2026-04-01", "2"))
                .unwrap();
        }
        let filter = |from: Option<&str>, to: Option<&str>| {
            store
                .exchange_rates(&RateFilter {
                    from: from.map(Into::into),
                    to: to.map(Into::into),
                })
                .unwrap()
                .len()
        };
        assert_eq!(filter(None, None), 3);
        assert_eq!(filter(Some("usd"), None), 2);
        assert_eq!(filter(None, Some("INR")), 2);
        assert_eq!(filter(Some("USD"), Some("INR")), 1);
    }
}
