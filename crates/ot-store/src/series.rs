// Derived from Frappe Books (AGPL-3.0)
// frappe_books/frappe_books/doctype/books_number_series and frappe_books/series.py
// @953e9b4, modified: the counter lives in `number_series.current_number`, taken in
// the same transaction as the document it names.

//! Number series in the book (roadmap step 1.4).

use duckdb::{Connection, OptionalExt, params};
use ot_core::series::{DEFAULT_PAD_ZEROS, DEFAULT_START, NumberSeries, SeriesError, SeriesType};
use serde::{Deserialize, Serialize};

use crate::store::{Store, StoreError};

/// The widest padding a series may use.
pub const MAX_PAD_ZEROS: u8 = 12;

/// A number series as the API shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SeriesRow {
    pub prefix: String,
    pub reference_type: SeriesType,
    /// The document type's name, e.g. "Sales Invoice".
    pub reference_label: &'static str,
    pub start: u64,
    pub pad_zeros: u8,
    /// The last number taken; `start - 1` while unused.
    pub current: u64,
    pub is_default: bool,
    /// The name the next document will get.
    pub next_name: String,
    /// Whether any document has taken a number.
    pub used: bool,
}

/// `POST /api/v1/series`.
#[derive(Debug, Clone, Deserialize)]
pub struct NewSeries {
    pub prefix: String,
    pub reference_type: SeriesType,
    #[serde(default)]
    pub start: Option<u64>,
    #[serde(default)]
    pub pad_zeros: Option<u8>,
    /// Make it the default series for its document type.
    #[serde(default)]
    pub make_default: bool,
}

/// `POST /api/v1/series/{prefix}`: only the fields given change.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct SeriesUpdate {
    #[serde(default)]
    pub start: Option<u64>,
    #[serde(default)]
    pub pad_zeros: Option<u8>,
    /// `true` makes it the default for its type. A default cannot be unset directly:
    /// make another series the default instead.
    #[serde(default)]
    pub make_default: Option<bool>,
}

fn invalid(error: SeriesError) -> StoreError {
    StoreError::Invalid(error.to_string())
}

fn check_pad(pad_zeros: u8) -> Result<u8, StoreError> {
    if (1..=MAX_PAD_ZEROS).contains(&pad_zeros) {
        Ok(pad_zeros)
    } else {
        Err(StoreError::Invalid(format!(
            "Pad zeros must be between 1 and {MAX_PAD_ZEROS}."
        )))
    }
}

fn check_start(start: u64) -> Result<u64, StoreError> {
    // Stored as BIGINT, and 0 would leave current at -1.
    if start == 0 || i64::try_from(start).is_err() {
        return Err(StoreError::Invalid(
            "Start must be a whole number from 1.".into(),
        ));
    }
    Ok(start)
}

const COLUMNS: &str = "prefix, reference_type, start_number, pad_zeros, current_number, is_default";

fn read_row(row: &duckdb::Row<'_>) -> duckdb::Result<(String, String, i64, i32, i64, bool)> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
    ))
}

fn to_series(
    raw: (String, String, i64, i32, i64, bool),
) -> Result<(NumberSeries, bool), StoreError> {
    let (prefix, kind, start, pad, current, is_default) = raw;
    let reference_type = SeriesType::from_key(&kind).ok_or_else(|| {
        StoreError::Invalid(format!(
            "Number series {prefix} has an unknown type {kind}."
        ))
    })?;
    let series = NumberSeries {
        prefix,
        reference_type,
        start: u64::try_from(start).unwrap_or(1),
        pad_zeros: u8::try_from(pad).unwrap_or(DEFAULT_PAD_ZEROS),
        current: u64::try_from(current).unwrap_or(0),
    };
    Ok((series, is_default))
}

fn to_row(series: NumberSeries, is_default: bool) -> SeriesRow {
    SeriesRow {
        next_name: series.peek().unwrap_or_default(),
        used: series.current >= series.start,
        reference_label: series.reference_type.label(),
        prefix: series.prefix,
        reference_type: series.reference_type,
        start: series.start,
        pad_zeros: series.pad_zeros,
        current: series.current,
        is_default,
    }
}

fn find(conn: &Connection, prefix: &str) -> Result<Option<(NumberSeries, bool)>, StoreError> {
    conn.query_row(
        &format!("SELECT {COLUMNS} FROM number_series WHERE prefix = ?"),
        params![prefix],
        read_row,
    )
    .optional()?
    .map(to_series)
    .transpose()
}

fn get(conn: &Connection, prefix: &str) -> Result<(NumberSeries, bool), StoreError> {
    find(conn, prefix)?
        .ok_or_else(|| StoreError::Missing(format!("Number series {prefix} not found.")))
}

fn make_default(conn: &Connection, series: &NumberSeries) -> Result<(), StoreError> {
    conn.execute(
        "UPDATE number_series SET is_default = (prefix = ?) WHERE reference_type = ?",
        params![series.prefix, series.reference_type.key()],
    )?;
    Ok(())
}

/// Takes the next name for a document of `doctype`, from `prefix` or else the type's
/// default series. Call it inside the transaction that saves the document, so a
/// failed save does not use up a number.
pub fn take_name(
    conn: &Connection,
    doctype: SeriesType,
    prefix: Option<&str>,
) -> Result<String, StoreError> {
    let prefix = match prefix {
        Some(prefix) => prefix.to_owned(),
        None => conn
            .query_row(
                "SELECT prefix FROM number_series WHERE reference_type = ? AND is_default",
                params![doctype.key()],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .ok_or_else(|| {
                StoreError::Invalid(format!(
                    "There is no default number series for {} documents.",
                    doctype.label()
                ))
            })?,
    };
    let (mut series, _) = get(conn, &prefix)?;
    let name = series.next_name(doctype).map_err(invalid)?;
    conn.execute(
        "UPDATE number_series SET current_number = ? WHERE prefix = ?",
        params![
            i64::try_from(series.current).map_err(|_| invalid(SeriesError::Exhausted {
                series: series.prefix.clone()
            }))?,
            series.prefix
        ],
    )?;
    Ok(name)
}

impl Store {
    /// Every number series, grouped by document type.
    pub fn number_series(&self) -> Result<Vec<SeriesRow>, StoreError> {
        let mut statement = self.connection.prepare(&format!(
            "SELECT {COLUMNS} FROM number_series ORDER BY reference_type, prefix"
        ))?;
        let raws = statement
            .query_map([], read_row)?
            .collect::<Result<Vec<_>, _>>()?;
        raws.into_iter()
            .map(|raw| to_series(raw).map(|(s, d)| to_row(s, d)))
            .collect()
    }

    pub fn add_number_series(&mut self, input: NewSeries) -> Result<SeriesRow, StoreError> {
        let prefix = input.prefix.trim().to_owned();
        let series = NumberSeries::with_start(
            &prefix,
            input.reference_type,
            check_start(input.start.unwrap_or(DEFAULT_START))?,
            check_pad(input.pad_zeros.unwrap_or(DEFAULT_PAD_ZEROS))?,
        )
        .map_err(invalid)?;
        let tx = self.connection.transaction()?;
        if find(&tx, &prefix)?.is_some() {
            return Err(StoreError::Conflict(format!(
                "Number series {prefix} already exists."
            )));
        }
        let has_default: bool = tx.query_row(
            "SELECT COUNT(*) > 0 FROM number_series WHERE reference_type = ? AND is_default",
            params![series.reference_type.key()],
            |row| row.get(0),
        )?;
        tx.execute(
            &format!("INSERT INTO number_series ({COLUMNS}) VALUES (?, ?, ?, ?, ?, FALSE)"),
            params![
                series.prefix,
                series.reference_type.key(),
                series.start as i64,
                i32::from(series.pad_zeros),
                series.current as i64,
            ],
        )?;
        // The first series of a type becomes its default, as there must be one.
        let is_default = input.make_default || !has_default;
        if is_default {
            make_default(&tx, &series)?;
        }
        tx.commit()?;
        Ok(to_row(series, is_default))
    }

    pub fn update_number_series(
        &mut self,
        prefix: &str,
        update: SeriesUpdate,
    ) -> Result<SeriesRow, StoreError> {
        let tx = self.connection.transaction()?;
        let (mut series, mut is_default) = get(&tx, prefix)?;
        if let Some(pad) = update.pad_zeros {
            series.pad_zeros = check_pad(pad)?;
        }
        if let Some(start) = update.start {
            let start = check_start(start)?;
            if series.current >= series.start && start <= series.current {
                // As Frappe's start_series: numbers already given out are never reused.
                return Err(StoreError::Invalid(format!(
                    "Numbers up to {} of {} are already used; start from {} or later.",
                    series.current,
                    series.prefix,
                    series.current + 1
                )));
            }
            series.start = start;
            series.current = start - 1;
        }
        match update.make_default {
            Some(true) => {
                make_default(&tx, &series)?;
                is_default = true;
            }
            Some(false) if is_default => {
                return Err(StoreError::Invalid(format!(
                    "Make another {} series the default instead.",
                    series.reference_type.label()
                )));
            }
            _ => {}
        }
        tx.execute(
            "UPDATE number_series SET start_number = ?, pad_zeros = ?, current_number = ?
             WHERE prefix = ?",
            params![
                series.start as i64,
                i32::from(series.pad_zeros),
                series.current as i64,
                series.prefix
            ],
        )?;
        tx.commit()?;
        Ok(to_row(series, is_default))
    }

    /// Deletes a series that has not named any document and is not a default.
    pub fn delete_number_series(&mut self, prefix: &str) -> Result<(), StoreError> {
        let tx = self.connection.transaction()?;
        let (series, is_default) = get(&tx, prefix)?;
        if is_default {
            return Err(StoreError::Invalid(format!(
                "{} is the default {} series; make another series the default first.",
                series.prefix,
                series.reference_type.label()
            )));
        }
        if series.current >= series.start {
            return Err(StoreError::Invalid(format!(
                "{} has already named documents, so it cannot be deleted.",
                series.prefix
            )));
        }
        tx.execute(
            "DELETE FROM number_series WHERE prefix = ?",
            params![prefix],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Takes the next document name in its own transaction. Documents should call
    /// [`take_name`] in their save transaction instead.
    pub fn take_series_name(
        &mut self,
        doctype: SeriesType,
        prefix: Option<&str>,
    ) -> Result<String, StoreError> {
        let tx = self.connection.transaction()?;
        let name = take_name(&tx, doctype, prefix)?;
        tx.commit()?;
        Ok(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Store {
        Store::open_in_memory().unwrap()
    }

    fn new(prefix: &str, kind: SeriesType) -> NewSeries {
        NewSeries {
            prefix: prefix.into(),
            reference_type: kind,
            start: None,
            pad_zeros: None,
            make_default: false,
        }
    }

    #[test]
    fn standard_series_are_seeded_as_defaults() {
        let store = store();
        let rows = store.number_series().unwrap();
        assert_eq!(rows.len(), SeriesType::ALL.len());
        for kind in SeriesType::ALL {
            let row = rows.iter().find(|r| r.reference_type == kind).unwrap();
            assert_eq!(row.prefix, kind.standard_prefix());
            assert!(row.is_default && !row.used);
            assert_eq!(row.next_name, format!("{}1001", kind.standard_prefix()));
        }
    }

    #[test]
    fn names_are_taken_from_the_default_or_a_chosen_series() {
        let mut store = store();
        let kind = SeriesType::SalesInvoice;
        assert_eq!(store.take_series_name(kind, None).unwrap(), "SINV-1001");
        assert_eq!(store.take_series_name(kind, None).unwrap(), "SINV-1002");
        let mut input = new("GST-26-", kind);
        input.start = Some(1);
        input.pad_zeros = Some(3);
        let added = store.add_number_series(input).unwrap();
        assert!(!added.is_default, "SINV- stays the default");
        assert_eq!(
            store.take_series_name(kind, Some("GST-26-")).unwrap(),
            "GST-26-001"
        );
        assert!(matches!(
            store.take_series_name(SeriesType::Payment, Some("GST-26-")),
            Err(StoreError::Invalid(_))
        ));
        let sinv = store
            .number_series()
            .unwrap()
            .into_iter()
            .find(|r| r.prefix == "SINV-")
            .unwrap();
        assert!(sinv.used);
        assert_eq!(sinv.next_name, "SINV-1003");
    }

    #[test]
    fn a_failed_save_does_not_use_a_number() {
        let mut store = store();
        {
            let tx = store.connection.transaction().unwrap();
            assert_eq!(
                take_name(&tx, SeriesType::Payment, None).unwrap(),
                "PAY-1001"
            );
            // Dropped without commit, as when the document fails to save.
        }
        assert_eq!(
            store.take_series_name(SeriesType::Payment, None).unwrap(),
            "PAY-1001"
        );
    }

    #[test]
    fn defaults_move_and_cannot_be_left_empty() {
        let mut store = store();
        let mut input = new("JV-26-", SeriesType::JournalEntry);
        input.make_default = true;
        assert!(store.add_number_series(input).unwrap().is_default);
        assert_eq!(
            store
                .take_series_name(SeriesType::JournalEntry, None)
                .unwrap(),
            "JV-26-1001"
        );
        let old = |store: &Store| {
            store
                .number_series()
                .unwrap()
                .into_iter()
                .find(|r| r.prefix == "JV-")
                .unwrap()
        };
        assert!(!old(&store).is_default);
        let unset = SeriesUpdate {
            make_default: Some(false),
            ..Default::default()
        };
        assert!(matches!(
            store.update_number_series("JV-26-", unset),
            Err(StoreError::Invalid(_))
        ));
        let set = SeriesUpdate {
            make_default: Some(true),
            ..Default::default()
        };
        assert!(store.update_number_series("JV-", set).unwrap().is_default);
        assert!(
            !store
                .number_series()
                .unwrap()
                .iter()
                .any(|r| r.prefix == "JV-26-" && r.is_default)
        );
    }

    #[test]
    fn start_never_reuses_numbers() {
        let mut store = store();
        let restart = |start| SeriesUpdate {
            start: Some(start),
            ..Default::default()
        };
        // Unused: the start may move either way.
        assert_eq!(
            store
                .update_number_series("SHPM-", restart(5))
                .unwrap()
                .next_name,
            "SHPM-0005"
        );
        store.take_series_name(SeriesType::Shipment, None).unwrap();
        assert!(matches!(
            store.update_number_series("SHPM-", restart(5)),
            Err(StoreError::Invalid(_))
        ));
        let moved = store.update_number_series("SHPM-", restart(100)).unwrap();
        assert_eq!(moved.next_name, "SHPM-0100");
        let padded = store
            .update_number_series(
                "SHPM-",
                SeriesUpdate {
                    pad_zeros: Some(6),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(padded.next_name, "SHPM-000100");
        assert!(matches!(
            store.update_number_series(
                "SHPM-",
                SeriesUpdate {
                    pad_zeros: Some(0),
                    ..Default::default()
                }
            ),
            Err(StoreError::Invalid(_))
        ));
    }

    #[test]
    fn add_validates_and_delete_only_unused_non_defaults() {
        let mut store = store();
        assert!(matches!(
            store.add_number_series(new("A/B", SeriesType::Payment)),
            Err(StoreError::Invalid(_))
        ));
        assert!(matches!(
            store.add_number_series(new("PAY-", SeriesType::Payment)),
            Err(StoreError::Conflict(_))
        ));
        let mut zero = new("Z-", SeriesType::Payment);
        zero.start = Some(0);
        assert!(matches!(
            store.add_number_series(zero),
            Err(StoreError::Invalid(_))
        ));
        assert!(matches!(
            store.delete_number_series("PAY-"),
            Err(StoreError::Invalid(_))
        ));
        store
            .add_number_series(new("RCPT-", SeriesType::Payment))
            .unwrap();
        store
            .take_series_name(SeriesType::Payment, Some("RCPT-"))
            .unwrap();
        assert!(matches!(
            store.delete_number_series("RCPT-"),
            Err(StoreError::Invalid(_))
        ));
        store
            .add_number_series(new("SPARE-", SeriesType::Payment))
            .unwrap();
        store.delete_number_series("SPARE-").unwrap();
        assert!(matches!(
            store.delete_number_series("SPARE-"),
            Err(StoreError::Missing(_))
        ));
    }
}
