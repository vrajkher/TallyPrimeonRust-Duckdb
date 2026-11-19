// Derived from Frappe Books (AGPL-3.0) frappe_books/accounting/ledger.py @953e9b4,
// modified: a pure Rust builder that returns balanced lines instead of inserting
// ledger entries. Cancellation stays with OpenTally's revision model.

//! Balanced general-ledger postings.
//!
//! A document adds debits and credits at full decimal precision. [`LedgerPosting::post`]
//! rounds each entry to the currency's smallest unit, absorbs the cents lost to rounding
//! in the round-off account (at most half a unit per entry), and refuses anything that
//! is still unbalanced.

use std::fmt;

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::money::{Currency, Money, MoneyError};

/// Accounts and parties are referred to by their ids in the book.
pub type AccountId = i64;
pub type PartyId = i64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Debit,
    Credit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EntryKey {
    pub account: AccountId,
    pub party: Option<PartyId>,
}

/// One rounded line of a balanced posting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PostingLine {
    pub account: AccountId,
    pub party: Option<PartyId>,
    pub side: Side,
    pub amount: Money,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Posting {
    /// Debits first, then credits, each in the order their account was first used.
    pub lines: Vec<PostingLine>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PostingError {
    /// "Ledger amounts cannot be negative."
    NegativeAmount,
    /// "Total debit {0} must equal total credit {1}."
    Unbalanced {
        debit: Money,
        credit: Money,
    },
    /// "Ledger posting total must be greater than zero."
    ZeroTotal,
    /// "Set a round-off account in Books Accounting Settings."
    NoRoundOffAccount,
    Money(MoneyError),
}

impl fmt::Display for PostingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NegativeAmount => f.write_str("Ledger amounts cannot be negative."),
            Self::Unbalanced { debit, credit } => {
                write!(f, "Total debit {debit} must equal total credit {credit}.")
            }
            Self::ZeroTotal => f.write_str("Ledger posting total must be greater than zero."),
            Self::NoRoundOffAccount => {
                f.write_str("Set a round-off account in the accounting settings.")
            }
            Self::Money(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for PostingError {}

impl From<MoneyError> for PostingError {
    fn from(error: MoneyError) -> Self {
        Self::Money(error)
    }
}

/// Collects a document's debits and credits; entries for the same account and party merge.
#[derive(Debug, Clone)]
pub struct LedgerPosting {
    currency: Currency,
    debits: Vec<(EntryKey, Decimal)>,
    credits: Vec<(EntryKey, Decimal)>,
}

impl LedgerPosting {
    pub fn new(currency: Currency) -> Self {
        Self {
            currency,
            debits: Vec::new(),
            credits: Vec::new(),
        }
    }

    pub fn currency(&self) -> &Currency {
        &self.currency
    }

    pub fn debit(
        &mut self,
        account: AccountId,
        amount: Decimal,
        party: Option<PartyId>,
    ) -> Result<&mut Self, PostingError> {
        add(&mut self.debits, EntryKey { account, party }, amount)?;
        Ok(self)
    }

    pub fn credit(
        &mut self,
        account: AccountId,
        amount: Decimal,
        party: Option<PartyId>,
    ) -> Result<&mut Self, PostingError> {
        add(&mut self.credits, EntryKey { account, party }, amount)?;
        Ok(self)
    }

    /// Adds an entry on the given side.
    pub fn entry(
        &mut self,
        side: Side,
        account: AccountId,
        amount: Decimal,
        party: Option<PartyId>,
    ) -> Result<&mut Self, PostingError> {
        match side {
            Side::Debit => self.debit(account, amount, party),
            Side::Credit => self.credit(account, amount, party),
        }
    }

    /// Every account the posting touches, for the caller to check they exist and are
    /// not group accounts.
    pub fn accounts(&self) -> impl Iterator<Item = AccountId> + '_ {
        self.debits
            .iter()
            .chain(&self.credits)
            .map(|(key, _)| key.account)
    }

    /// Rounds, balances and returns the lines. `round_off` receives the rounding
    /// difference, if there is one.
    pub fn post(&self, round_off: Option<AccountId>) -> Result<Posting, PostingError> {
        let mut debits = self.rounded(&self.debits)?;
        let mut credits = self.rounded(&self.credits)?;
        self.add_round_off(&mut debits, &mut credits, round_off)?;

        let debit = total(&debits);
        let credit = total(&credits);
        if debit != credit {
            return Err(self.unbalanced(debit, credit)?);
        }
        if debit == 0 {
            return Err(PostingError::ZeroTotal);
        }

        let mut lines = Vec::with_capacity(debits.len() + credits.len());
        for (side, entries) in [(Side::Debit, debits), (Side::Credit, credits)] {
            for (key, minor) in entries {
                lines.push(PostingLine {
                    account: key.account,
                    party: key.party,
                    side,
                    amount: Money::from_minor(minor, self.currency.clone())?,
                });
            }
        }
        Ok(Posting { lines })
    }

    /// Each entry rounded on its own; entries that round to zero are dropped.
    fn rounded(
        &self,
        entries: &[(EntryKey, Decimal)],
    ) -> Result<Vec<(EntryKey, i128)>, PostingError> {
        let mut rounded = Vec::with_capacity(entries.len());
        for (key, amount) in entries {
            let minor = self.currency.round(*amount)?.minor();
            if minor != 0 {
                rounded.push((*key, minor));
            }
        }
        Ok(rounded)
    }

    /// Absorbs the cents lost when each entry is rounded on its own.
    fn add_round_off(
        &self,
        debits: &mut Vec<(EntryKey, i128)>,
        credits: &mut Vec<(EntryKey, i128)>,
        round_off: Option<AccountId>,
    ) -> Result<(), PostingError> {
        let difference = total(debits) - total(credits);
        if difference == 0 {
            return Ok(());
        }
        // Rounding moves each entry by at most half the smallest unit.
        let entries = (debits.len() + credits.len()) as i128;
        if difference.abs() * 2 > entries {
            return Err(self.unbalanced(total(debits), total(credits))?);
        }
        let account = round_off.ok_or(PostingError::NoRoundOffAccount)?;
        let side = if difference > 0 { credits } else { debits };
        let key = EntryKey {
            account,
            party: None,
        };
        match side.iter_mut().find(|(k, _)| *k == key) {
            Some((_, minor)) => *minor += difference.abs(),
            None => side.push((key, difference.abs())),
        }
        Ok(())
    }

    fn unbalanced(&self, debit: i128, credit: i128) -> Result<PostingError, PostingError> {
        Ok(PostingError::Unbalanced {
            debit: Money::from_minor(debit, self.currency.clone())?,
            credit: Money::from_minor(credit, self.currency.clone())?,
        })
    }
}

fn add(
    entries: &mut Vec<(EntryKey, Decimal)>,
    key: EntryKey,
    amount: Decimal,
) -> Result<(), PostingError> {
    if amount.is_sign_negative() && !amount.is_zero() {
        return Err(PostingError::NegativeAmount);
    }
    match entries.iter_mut().find(|(k, _)| *k == key) {
        Some((_, total)) => *total += amount,
        None => entries.push((key, amount)),
    }
    Ok(())
}

fn total(entries: &[(EntryKey, i128)]) -> i128 {
    entries.iter().map(|(_, minor)| minor).sum()
}

impl Posting {
    pub fn total_debit(&self) -> i128 {
        self.side_total(Side::Debit)
    }

    pub fn total_credit(&self) -> i128 {
        self.side_total(Side::Credit)
    }

    fn side_total(&self, side: Side) -> i128 {
        self.lines
            .iter()
            .filter(|line| line.side == side)
            .map(|line| line.amount.minor())
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;

    const CASH: AccountId = 1;
    const INCOME: AccountId = 2;
    const TAX: AccountId = 3;
    const ROUND_OFF: AccountId = 9;

    fn dec(text: &str) -> Decimal {
        Decimal::from_str(text).unwrap()
    }

    fn inr() -> LedgerPosting {
        LedgerPosting::new(Currency::inr())
    }

    // Ported from tests/test_ledger.py::test_rejects_gap_larger_than_rounding.
    #[test]
    fn rejects_gap_larger_than_rounding() {
        let mut posting = inr();
        posting.debit(CASH, dec("10"), None).unwrap();
        posting.credit(INCOME, dec("9.50"), None).unwrap();
        let error = posting.post(Some(ROUND_OFF)).unwrap_err();
        assert!(matches!(error, PostingError::Unbalanced { .. }));
        assert_eq!(
            error.to_string(),
            "Total debit 10.00 must equal total credit 9.50."
        );
    }

    #[test]
    fn balanced_posting_keeps_order_and_merges_entries() {
        let mut posting = inr();
        posting.debit(CASH, dec("100"), None).unwrap();
        posting.credit(INCOME, dec("80"), None).unwrap();
        posting.credit(TAX, dec("10"), None).unwrap();
        posting.credit(TAX, dec("10"), None).unwrap();
        let lines = posting.post(None).unwrap().lines;
        let summary: Vec<_> = lines
            .iter()
            .map(|l| (l.account, l.side, l.amount.minor()))
            .collect();
        assert_eq!(
            summary,
            vec![
                (CASH, Side::Debit, 10000),
                (INCOME, Side::Credit, 8000),
                (TAX, Side::Credit, 2000)
            ]
        );
    }

    #[test]
    fn rounding_difference_goes_to_round_off_account() {
        // Three thirds of 100: each rounds to 33.33, losing 0.01 overall.
        let mut posting = inr();
        posting.debit(CASH, dec("100"), None).unwrap();
        for account in [INCOME, TAX, 4] {
            posting
                .credit(account, dec("100") / dec("3"), None)
                .unwrap();
        }
        let result = posting.post(Some(ROUND_OFF)).unwrap();
        assert_eq!(result.total_debit(), result.total_credit());
        let round_off = result.lines.last().unwrap();
        assert_eq!(
            (round_off.account, round_off.side, round_off.amount.minor()),
            (ROUND_OFF, Side::Credit, 1)
        );
    }

    #[test]
    fn round_off_on_the_debit_side_when_credits_are_larger() {
        let mut posting = inr();
        posting.credit(INCOME, dec("100"), None).unwrap();
        for account in [CASH, TAX, 4] {
            posting.debit(account, dec("33.334"), None).unwrap();
        }
        // Debits round to 3 × 33.33 = 99.99.
        let result = posting.post(Some(ROUND_OFF)).unwrap();
        let round_off = result
            .lines
            .iter()
            .find(|l| l.account == ROUND_OFF)
            .unwrap();
        assert_eq!((round_off.side, round_off.amount.minor()), (Side::Debit, 1));
    }

    #[test]
    fn rounding_difference_needs_a_round_off_account() {
        let mut posting = inr();
        posting.debit(CASH, dec("100"), None).unwrap();
        for account in [INCOME, TAX, 4] {
            posting
                .credit(account, dec("100") / dec("3"), None)
                .unwrap();
        }
        assert_eq!(posting.post(None), Err(PostingError::NoRoundOffAccount));
    }

    #[test]
    fn negative_and_zero_postings_are_refused() {
        let mut posting = inr();
        assert_eq!(
            posting.debit(CASH, dec("-1"), None).unwrap_err(),
            PostingError::NegativeAmount
        );
        posting.debit(CASH, dec("0.001"), None).unwrap();
        posting.credit(INCOME, dec("0.004"), None).unwrap();
        assert_eq!(posting.post(Some(ROUND_OFF)), Err(PostingError::ZeroTotal));
    }

    #[test]
    fn parties_keep_separate_lines() {
        let mut posting = inr();
        posting.debit(CASH, dec("50"), Some(7)).unwrap();
        posting.debit(CASH, dec("50"), Some(8)).unwrap();
        posting.credit(INCOME, dec("100"), None).unwrap();
        let lines = posting.post(None).unwrap().lines;
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].party, Some(7));
        assert_eq!(lines[1].party, Some(8));
    }

    #[test]
    fn zero_precision_currency_rounds_to_whole_units() {
        let mut posting = LedgerPosting::new(Currency::new("JPY").unwrap());
        posting.debit(CASH, dec("1000.4"), None).unwrap();
        posting.credit(INCOME, dec("1000"), None).unwrap();
        let lines = posting.post(None).unwrap().lines;
        assert_eq!(lines[0].amount.minor(), 1000);
    }
}
