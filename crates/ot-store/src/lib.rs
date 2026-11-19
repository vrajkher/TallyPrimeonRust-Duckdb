//! OpenTally book storage: the DuckDB schema, migrations, vouchers, reports,
//! imports and bank reconciliation.

#[cfg(test)]
mod acceptance;
pub mod auth;
pub mod bank;
pub mod imports;
pub mod migrate;
pub mod reports;
mod store;

pub use store::*;
