// Derived from Frappe Books (AGPL-3.0) DocType permissions and
// frappe_books/permissions.py @953e9b4, modified: a role table in Rust, a Viewer
// role, and API-token scopes that narrow what a token's user may do.

//! Who may do what: roles, per-DocType permissions and API-token scopes.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::permissions_table::DOCTYPE_PERMISSIONS;

/// A user's role. The first four are Frappe Books' staff roles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// Frappe's System Manager: everything, including users and tokens.
    SystemManager,
    /// Books Manager: all accounting, settings and masters.
    BooksManager,
    /// Books User: sales, purchases and payments; no journals or accounts.
    BooksUser,
    /// Books Cashier: point of sale only.
    BooksCashier,
    /// Read-only access to everything Books Manager can read.
    Viewer,
}

impl Role {
    pub const ALL: [Self; 5] = [
        Self::SystemManager,
        Self::BooksManager,
        Self::BooksUser,
        Self::BooksCashier,
        Self::Viewer,
    ];

    /// The key stored in the book.
    pub fn key(self) -> &'static str {
        match self {
            Self::SystemManager => "system_manager",
            Self::BooksManager => "books_manager",
            Self::BooksUser => "books_user",
            Self::BooksCashier => "books_cashier",
            Self::Viewer => "viewer",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|role| role.key() == key)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::SystemManager => "System Manager",
            Self::BooksManager => "Books Manager",
            Self::BooksUser => "Books User",
            Self::BooksCashier => "Books Cashier",
            Self::Viewer => "Viewer",
        }
    }
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// A Frappe permission. The order matches the bits in the generated table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Perm {
    Read,
    Write,
    Create,
    Delete,
    Submit,
    Cancel,
    Amend,
    Report,
    Import,
    Export,
    Print,
}

impl Perm {
    const ALL: [Self; 11] = [
        Self::Read,
        Self::Write,
        Self::Create,
        Self::Delete,
        Self::Submit,
        Self::Cancel,
        Self::Amend,
        Self::Report,
        Self::Import,
        Self::Export,
        Self::Print,
    ];

    fn bit(self) -> u16 {
        1 << Self::ALL.iter().position(|p| *p == self).expect("listed")
    }

    /// Permissions that never change the book.
    pub fn is_read_only(self) -> bool {
        matches!(self, Self::Read | Self::Report | Self::Export | Self::Print)
    }
}

/// How far an API token may act for its user. A token never has more rights than its user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    /// Read, report, export and print.
    Read,
    /// Also create and edit drafts.
    Draft,
    /// Also submit, cancel, amend, delete and import.
    Submit,
    /// Also manage users and tokens.
    Admin,
}

impl Scope {
    pub fn key(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Draft => "draft",
            Self::Submit => "submit",
            Self::Admin => "admin",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        [Self::Read, Self::Draft, Self::Submit, Self::Admin]
            .into_iter()
            .find(|scope| scope.key() == key)
    }

    fn allows(self, perm: Perm) -> bool {
        match perm {
            Perm::Read | Perm::Report | Perm::Export | Perm::Print => true,
            Perm::Create | Perm::Write => self >= Self::Draft,
            Perm::Submit | Perm::Cancel | Perm::Amend | Perm::Delete | Perm::Import => {
                self >= Self::Submit
            }
        }
    }
}

/// Whether `role` has `perm` on `doctype`, by Frappe Books' rules.
pub fn role_allows(role: Role, doctype: &str, perm: Perm) -> bool {
    let Some((_, _, _, masks)) = DOCTYPE_PERMISSIONS
        .iter()
        .find(|(name, ..)| *name == doctype)
    else {
        return false;
    };
    match role {
        Role::SystemManager => masks[0] & perm.bit() != 0,
        Role::BooksManager => masks[1] & perm.bit() != 0,
        Role::BooksUser => masks[2] & perm.bit() != 0,
        Role::BooksCashier => masks[3] & perm.bit() != 0,
        Role::Viewer => perm.is_read_only() && masks[1] & perm.bit() != 0,
    }
}

/// The DocTypes Frappe Books defines, for listing and checks.
pub fn doctypes() -> impl Iterator<Item = &'static str> {
    DOCTYPE_PERMISSIONS.iter().map(|(name, ..)| *name)
}

/// The rights of whoever is making a request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Grant {
    pub roles: Vec<Role>,
    /// `None` for a signed-in person; a token's scope otherwise.
    pub scope: Option<Scope>,
}

impl Grant {
    pub fn new(roles: Vec<Role>, scope: Option<Scope>) -> Self {
        Self { roles, scope }
    }

    pub fn can(&self, doctype: &str, perm: Perm) -> bool {
        self.scope.is_none_or(|scope| scope.allows(perm))
            && self
                .roles
                .iter()
                .any(|role| role_allows(*role, doctype, perm))
    }

    /// Managing users and tokens of other users.
    pub fn is_admin(&self) -> bool {
        self.roles.contains(&Role::SystemManager) && self.scope.is_none_or(|s| s == Scope::Admin)
    }
}

/// The shortest password a user may set.
pub const MIN_PASSWORD_LEN: usize = 10;

/// A user name: 1–64 characters of letters, digits, `.`, `_`, `-` or `@`.
pub fn valid_username(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '@'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_matches_frappe_books_rules() {
        // Journal entries and accounts are for managers only.
        assert!(role_allows(
            Role::BooksManager,
            "Books Journal Entry",
            Perm::Submit
        ));
        assert!(!role_allows(
            Role::BooksUser,
            "Books Journal Entry",
            Perm::Read
        ));
        assert!(!role_allows(Role::BooksUser, "Books Account", Perm::Read));
        // Books Users take payments but cannot delete them or import.
        assert!(role_allows(Role::BooksUser, "Books Payment", Perm::Submit));
        assert!(!role_allows(Role::BooksUser, "Books Payment", Perm::Delete));
        assert!(!role_allows(Role::BooksUser, "Books Payment", Perm::Import));
        // Cashiers only read payments, and sell through the POS.
        assert!(!role_allows(
            Role::BooksCashier,
            "Books Payment",
            Perm::Create
        ));
        assert!(role_allows(
            Role::BooksCashier,
            "Books Sales Invoice",
            Perm::Submit
        ));
        assert!(!role_allows(
            Role::BooksCashier,
            "Books Sales Invoice",
            Perm::Cancel
        ));
        // Ledger entries are written only by postings.
        assert!(!role_allows(
            Role::SystemManager,
            "Books Ledger Entry",
            Perm::Write
        ));
        assert!(!role_allows(
            Role::SystemManager,
            "No Such DocType",
            Perm::Read
        ));
        assert!(doctypes().count() > 40);
    }

    #[test]
    fn viewer_reads_what_a_manager_reads_and_changes_nothing() {
        assert!(role_allows(Role::Viewer, "Books Journal Entry", Perm::Read));
        assert!(role_allows(
            Role::Viewer,
            "Books Ledger Entry",
            Perm::Report
        ));
        assert!(!role_allows(
            Role::Viewer,
            "Books Journal Entry",
            Perm::Create
        ));
        assert!(!role_allows(Role::Viewer, "Books Party", Perm::Write));
    }

    #[test]
    fn token_scope_narrows_the_user() {
        let manager = vec![Role::BooksManager];
        let read = Grant::new(manager.clone(), Some(Scope::Read));
        assert!(read.can("Books Journal Entry", Perm::Read));
        assert!(!read.can("Books Journal Entry", Perm::Create));
        let draft = Grant::new(manager.clone(), Some(Scope::Draft));
        assert!(draft.can("Books Journal Entry", Perm::Create));
        assert!(!draft.can("Books Journal Entry", Perm::Submit));
        let submit = Grant::new(manager.clone(), Some(Scope::Submit));
        assert!(submit.can("Books Journal Entry", Perm::Submit));
        assert!(!submit.is_admin());
        // A scope never adds rights the user lacks.
        let user = Grant::new(vec![Role::BooksUser], Some(Scope::Admin));
        assert!(!user.can("Books Journal Entry", Perm::Read));
        assert!(!user.is_admin());
        assert!(Grant::new(vec![Role::SystemManager], None).is_admin());
        assert!(!Grant::new(vec![Role::SystemManager], Some(Scope::Submit)).is_admin());
    }

    #[test]
    fn keys_round_trip() {
        for role in Role::ALL {
            assert_eq!(Role::from_key(role.key()), Some(role));
        }
        for scope in [Scope::Read, Scope::Draft, Scope::Submit, Scope::Admin] {
            assert_eq!(Scope::from_key(scope.key()), Some(scope));
        }
        assert!(valid_username("asha.k@shop"));
        assert!(!valid_username("a b"));
        assert!(!valid_username(""));
    }
}
