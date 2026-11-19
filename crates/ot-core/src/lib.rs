//! OpenTally's accounting rules, free of storage and I/O.
//!
//! Parts of this crate are derived from Frappe Books (AGPL-3.0); each such module
//! names its upstream source. See `NOTICE`.

pub mod doc;
pub mod ledger;
pub mod money;
pub mod series;

pub use rust_decimal::Decimal;
