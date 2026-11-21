//! OpenTally book storage: the DuckDB schema, migrations, vouchers, reports,
//! imports and bank reconciliation.

#[cfg(test)]
mod acceptance;
pub mod auth;
pub mod bank;
pub mod chart;
pub mod exchange_rates;
pub mod imports;
pub mod items;
pub mod migrate;
pub mod parties;
pub mod payment_methods;
pub mod reports;
pub mod series;
mod store;
pub mod taxes;

pub use store::*;
