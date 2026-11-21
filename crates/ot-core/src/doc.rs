// Derived from Frappe Books (AGPL-3.0) frappe_books/status.py and
// frappe_books/linked_entries.py @953e9b4, and from the Frappe Framework's
// docstatus and set-only-once rules, modified: pure Rust functions over plain inputs.

//! The life of a document: draft, submitted, cancelled; its list status; fields that
//! can be set only once; and the documents that link to it.

use std::{collections::BTreeMap, fmt};

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// Frappe's `docstatus`: 0 draft, 1 submitted, 2 cancelled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DocStatus {
    Draft,
    Submitted,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocError {
    /// A submitted or cancelled document cannot be edited.
    NotEditable(DocStatus),
    /// The action is not allowed from the document's current state.
    InvalidTransition { from: DocStatus, action: Action },
    /// A set-once field already has a saved value.
    CannotChange { field: &'static str },
    /// Linked documents must be cancelled first.
    HasLinkedDocuments(LinkedEntries),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Save,
    Submit,
    Cancel,
    Amend,
}

impl fmt::Display for DocError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotEditable(status) => write!(f, "A {status} document cannot be changed."),
            Self::InvalidTransition { from, action } => {
                write!(f, "A {from} document cannot be {}.", action.past_tense())
            }
            Self::CannotChange { field } => write!(f, "{field} cannot be changed once saved."),
            Self::HasLinkedDocuments(linked) => write!(
                f,
                "Cancel the linked documents first: {}.",
                linked.summary()
            ),
        }
    }
}

impl std::error::Error for DocError {}

impl Action {
    fn past_tense(self) -> &'static str {
        match self {
            Self::Save => "saved",
            Self::Submit => "submitted",
            Self::Cancel => "cancelled",
            Self::Amend => "amended",
        }
    }
}

impl fmt::Display for DocStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Draft => "draft",
            Self::Submitted => "submitted",
            Self::Cancelled => "cancelled",
        })
    }
}

impl DocStatus {
    pub fn as_i32(self) -> i32 {
        match self {
            Self::Draft => 0,
            Self::Submitted => 1,
            Self::Cancelled => 2,
        }
    }

    pub fn from_i32(value: i32) -> Option<Self> {
        match value {
            0 => Some(Self::Draft),
            1 => Some(Self::Submitted),
            2 => Some(Self::Cancelled),
            _ => None,
        }
    }

    /// Only drafts can be edited and saved again.
    pub fn check_save(self) -> Result<(), DocError> {
        match self {
            Self::Draft => Ok(()),
            other => Err(DocError::NotEditable(other)),
        }
    }

    /// Draft → Submitted.
    pub fn submit(self) -> Result<Self, DocError> {
        self.transition(Action::Submit, Self::Draft, Self::Submitted)
    }

    /// Submitted → Cancelled, when no live document links to it.
    pub fn cancel(self, linked: &LinkedEntries) -> Result<Self, DocError> {
        let next = self.transition(Action::Cancel, Self::Submitted, Self::Cancelled)?;
        if !linked.is_empty() {
            return Err(DocError::HasLinkedDocuments(linked.clone()));
        }
        Ok(next)
    }

    /// A cancelled document can be amended into a new draft that records `amended_from`.
    pub fn amend(self) -> Result<Self, DocError> {
        self.transition(Action::Amend, Self::Cancelled, Self::Draft)
    }

    fn transition(self, action: Action, from: Self, to: Self) -> Result<Self, DocError> {
        if self == from {
            Ok(to)
        } else {
            Err(DocError::InvalidTransition { from: self, action })
        }
    }
}

/// A field set when the document is first saved and never changed afterwards, as
/// Frappe's `set_only_once`. `saved` is `None` for a document not yet saved.
pub fn check_set_once<T: PartialEq>(
    field: &'static str,
    saved: Option<&T>,
    new: &T,
) -> Result<(), DocError> {
    match saved {
        Some(saved) if saved != new => Err(DocError::CannotChange { field }),
        _ => Ok(()),
    }
}

/// The status shown in lists, as Frappe Books stores it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ListStatus {
    Saved,
    Submitted,
    Cancelled,
    Return,
    #[serde(rename = "Return Issued")]
    ReturnIssued,
    Paid,
    Unpaid,
    #[serde(rename = "Partly Paid")]
    PartlyPaid,
    Open,
    Closed,
}

impl fmt::Display for ListStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Saved => "Saved",
            Self::Submitted => "Submitted",
            Self::Cancelled => "Cancelled",
            Self::Return => "Return",
            Self::ReturnIssued => "Return Issued",
            Self::Paid => "Paid",
            Self::Unpaid => "Unpaid",
            Self::PartlyPaid => "Partly Paid",
            Self::Open => "Open",
            Self::Closed => "Closed",
        })
    }
}

/// What the list status depends on, by kind of document.
#[derive(Debug, Clone, PartialEq)]
pub enum StatusFacts {
    Invoice {
        outstanding: Decimal,
        base_grand_total: Decimal,
        is_returned: bool,
        is_fully_returned: bool,
    },
    PosShift {
        closed: bool,
    },
    Other {
        is_returned: bool,
    },
}

/// The list status of a submittable document. `is_return` is true when the document
/// is itself a return against another one.
pub fn list_status(docstatus: DocStatus, is_return: bool, facts: &StatusFacts) -> ListStatus {
    match docstatus {
        DocStatus::Draft => return ListStatus::Saved,
        DocStatus::Cancelled => return ListStatus::Cancelled,
        DocStatus::Submitted => {}
    }
    match facts {
        StatusFacts::PosShift { closed } => {
            if *closed {
                ListStatus::Closed
            } else {
                ListStatus::Open
            }
        }
        _ if is_return => ListStatus::Return,
        StatusFacts::Invoice {
            outstanding,
            base_grand_total,
            is_returned,
            is_fully_returned,
        } => {
            // A fully returned invoice, or a settled one with a return, shows the return.
            let settled = outstanding.is_zero();
            if *is_fully_returned || (*is_returned && settled) {
                ListStatus::ReturnIssued
            } else if settled {
                ListStatus::Paid
            } else if outstanding == base_grand_total {
                ListStatus::Unpaid
            } else {
                ListStatus::PartlyPaid
            }
        }
        StatusFacts::Other { is_returned: true } => ListStatus::ReturnIssued,
        StatusFacts::Other { is_returned: false } => ListStatus::Submitted,
    }
}

/// At most this many linked documents are listed per doctype.
pub const LINKED_ENTRIES_LIMIT: usize = 100;

/// Live (not cancelled) documents that link to a document, by doctype, newest first.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkedEntries(pub BTreeMap<String, Vec<String>>);

impl LinkedEntries {
    /// Builds the list from `(doctype, name, created)` rows, newest first and capped.
    pub fn from_rows<C: Ord>(rows: impl IntoIterator<Item = (String, String, C)>) -> Self {
        let mut by_doctype: BTreeMap<String, Vec<(C, String)>> = BTreeMap::new();
        for (doctype, name, created) in rows {
            let entries = by_doctype.entry(doctype).or_default();
            if !entries.iter().any(|(_, n)| *n == name) {
                entries.push((created, name));
            }
        }
        Self(
            by_doctype
                .into_iter()
                .map(|(doctype, mut entries)| {
                    entries.sort_by(|a, b| b.0.cmp(&a.0));
                    entries.truncate(LINKED_ENTRIES_LIMIT);
                    (doctype, entries.into_iter().map(|(_, n)| n).collect())
                })
                .collect(),
        )
    }

    pub fn is_empty(&self) -> bool {
        self.0.values().all(Vec::is_empty)
    }

    fn summary(&self) -> String {
        self.0
            .iter()
            .filter(|(_, names)| !names.is_empty())
            .map(|(doctype, names)| format!("{doctype} {}", names.join(", ")))
            .collect::<Vec<_>>()
            .join("; ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn invoice(outstanding: i64, total: i64, returned: bool, fully: bool) -> StatusFacts {
        StatusFacts::Invoice {
            outstanding: Decimal::from(outstanding),
            base_grand_total: Decimal::from(total),
            is_returned: returned,
            is_fully_returned: fully,
        }
    }

    #[test]
    fn documents_move_draft_submitted_cancelled_amended() {
        let none = LinkedEntries::default();
        let submitted = DocStatus::Draft.submit().unwrap();
        assert_eq!(submitted, DocStatus::Submitted);
        assert_eq!(
            submitted.check_save(),
            Err(DocError::NotEditable(DocStatus::Submitted))
        );
        assert!(submitted.submit().is_err());
        let cancelled = submitted.cancel(&none).unwrap();
        assert!(cancelled.cancel(&none).is_err());
        assert_eq!(cancelled.amend().unwrap(), DocStatus::Draft);
        assert!(DocStatus::Draft.cancel(&none).is_err());
        assert!(DocStatus::Draft.amend().is_err());
        for status in [DocStatus::Draft, DocStatus::Submitted, DocStatus::Cancelled] {
            assert_eq!(DocStatus::from_i32(status.as_i32()), Some(status));
        }
    }

    #[test]
    fn linked_documents_block_cancel() {
        let linked = LinkedEntries::from_rows([("Payment".to_string(), "PAY-1001".to_string(), 1)]);
        let error = DocStatus::Submitted.cancel(&linked).unwrap_err();
        assert_eq!(
            error.to_string(),
            "Cancel the linked documents first: Payment PAY-1001."
        );
    }

    // Ported from tests/test_status.py::test_invoice_status_follows_payments.
    #[test]
    fn invoice_status_follows_payments() {
        assert_eq!(
            list_status(DocStatus::Draft, false, &invoice(180, 180, false, false)),
            ListStatus::Saved
        );
        assert_eq!(
            list_status(
                DocStatus::Submitted,
                false,
                &invoice(180, 180, false, false)
            ),
            ListStatus::Unpaid
        );
        assert_eq!(
            list_status(
                DocStatus::Submitted,
                false,
                &invoice(100, 180, false, false)
            ),
            ListStatus::PartlyPaid
        );
        assert_eq!(
            list_status(DocStatus::Submitted, false, &invoice(0, 180, false, false)),
            ListStatus::Paid
        );
        assert_eq!(
            list_status(DocStatus::Cancelled, false, &invoice(0, 180, false, false)),
            ListStatus::Cancelled
        );
    }

    // Ported from tests/test_status.py::test_returns_mark_both_invoices and
    // test_a_partly_returned_invoice_shows_what_it_owes_until_settled.
    #[test]
    fn returns_mark_both_invoices() {
        assert_eq!(
            list_status(DocStatus::Submitted, true, &invoice(0, -180, false, false)),
            ListStatus::Return
        );
        assert_eq!(
            list_status(DocStatus::Submitted, false, &invoice(0, 180, true, true)),
            ListStatus::ReturnIssued
        );
        // Partly returned: shows what it owes until settled.
        assert_eq!(
            list_status(DocStatus::Submitted, false, &invoice(180, 270, true, false)),
            ListStatus::PartlyPaid
        );
        assert_eq!(
            list_status(DocStatus::Submitted, false, &invoice(0, 270, true, false)),
            ListStatus::ReturnIssued
        );
    }

    #[test]
    fn other_documents_and_pos_shifts() {
        assert_eq!(
            list_status(
                DocStatus::Submitted,
                false,
                &StatusFacts::Other { is_returned: false }
            ),
            ListStatus::Submitted
        );
        assert_eq!(
            list_status(
                DocStatus::Submitted,
                false,
                &StatusFacts::Other { is_returned: true }
            ),
            ListStatus::ReturnIssued
        );
        assert_eq!(
            list_status(
                DocStatus::Submitted,
                true,
                &StatusFacts::Other { is_returned: false }
            ),
            ListStatus::Return
        );
        assert_eq!(
            list_status(
                DocStatus::Submitted,
                false,
                &StatusFacts::PosShift { closed: false }
            ),
            ListStatus::Open
        );
        assert_eq!(
            list_status(
                DocStatus::Submitted,
                false,
                &StatusFacts::PosShift { closed: true }
            ),
            ListStatus::Closed
        );
        assert_eq!(ListStatus::PartlyPaid.to_string(), "Partly Paid");
    }

    // Ported from tests/test_set_once_fields.py: after insert, even an empty or false
    // value cannot change (is_group 0 → 1, track_item 0 → 1).
    #[test]
    fn set_once_fields_cannot_change_after_insert() {
        assert!(check_set_once("Root type", None, &"Asset").is_ok());
        assert!(check_set_once("Root type", Some(&"Asset"), &"Asset").is_ok());
        assert_eq!(
            check_set_once("Root type", Some(&"Asset"), &"Expense"),
            Err(DocError::CannotChange { field: "Root type" })
        );
        assert!(check_set_once("Is group", Some(&false), &true).is_err());
        assert!(check_set_once("Parent account", Some(&None), &Some(4_i64)).is_err());
    }

    #[test]
    fn linked_entries_are_newest_first_and_unique() {
        let linked = LinkedEntries::from_rows([
            ("Payment".to_string(), "PAY-1".to_string(), 1),
            ("Payment".to_string(), "PAY-2".to_string(), 3),
            ("Payment".to_string(), "PAY-2".to_string(), 3),
            ("Shipment".to_string(), "SHPM-1".to_string(), 2),
        ]);
        assert_eq!(linked.0["Payment"], vec!["PAY-2", "PAY-1"]);
        assert_eq!(linked.0["Shipment"], vec!["SHPM-1"]);
        assert!(LinkedEntries::default().is_empty());
    }
}
