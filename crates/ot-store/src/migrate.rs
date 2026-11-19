//! Numbered, checksummed schema migrations.
//!
//! Each migration runs once, inside the opening transaction, and is recorded in
//! `schema_migrations` with its checksum. A migration that changes after it was
//! applied stops the book from opening, so released migrations must never be edited.

use std::{
    fs,
    path::{Path, PathBuf},
};

use duckdb::{AccessMode, Config, Connection, Transaction};

/// One schema version, applied as its SQL parts in order.
pub struct Migration {
    pub version: i32,
    pub name: &'static str,
    pub parts: &'static [&'static str],
}

/// The DuckDB migrations, oldest first. Append new versions; never edit applied ones.
pub const MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    name: "initial",
    parts: &[
        include_str!("../migrations/duckdb/0001_initial/1_legacy.sql"),
        include_str!("../migrations/duckdb/0001_initial/2_schema.sql"),
        include_str!("../migrations/duckdb/0001_initial/3_upgrade_voucher_desk.sql"),
    ],
}];

impl Migration {
    /// FNV-1a over the SQL parts, as 16 hex digits. It detects edits, not tampering.
    pub fn checksum(&self) -> String {
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for part in self.parts {
            for byte in part.bytes().chain([0]) {
                hash ^= u64::from(byte);
                hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
            }
        }
        format!("{hash:016x}")
    }
}

#[derive(Debug)]
pub enum MigrationError {
    Database(duckdb::Error),
    Io(std::io::Error),
    /// The book records a migration whose SQL differs from this build's.
    Changed { version: i32 },
    /// The book was migrated by a newer OpenTally.
    TooNew { version: i32 },
}

impl std::fmt::Display for MigrationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Database(error) => write!(f, "database error while migrating: {error}"),
            Self::Io(error) => write!(f, "could not back up the book before migrating: {error}"),
            Self::Changed { version } => write!(
                f,
                "schema migration {version} differs from the one recorded in this book"
            ),
            Self::TooNew { version } => write!(
                f,
                "this book uses schema version {version}, which is newer than this OpenTally"
            ),
        }
    }
}

impl std::error::Error for MigrationError {}

impl From<duckdb::Error> for MigrationError {
    fn from(error: duckdb::Error) -> Self {
        Self::Database(error)
    }
}

impl From<std::io::Error> for MigrationError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

/// The highest schema version recorded in an existing book file, without changing it.
/// A file with no `schema_migrations` table (a Voucher Desk book) is version 0.
pub fn file_version(path: &Path) -> Result<i32, MigrationError> {
    let config = Config::default().access_mode(AccessMode::ReadOnly)?;
    let connection = Connection::open_with_flags(path, config)?;
    let has_table: bool = connection.query_row(
        "SELECT count(*) > 0 FROM information_schema.tables WHERE table_name = 'schema_migrations'",
        [],
        |row| row.get(0),
    )?;
    if !has_table {
        return Ok(0);
    }
    Ok(connection.query_row(
        "SELECT coalesce(max(version), 0) FROM schema_migrations",
        [],
        |row| row.get(0),
    )?)
}

/// Copies an existing book (and its WAL) to `<file>.bak-v<version>` when this build
/// has migrations it has not applied yet. Returns the backup path, if one was made.
pub fn backup_before_migrating(path: &Path) -> Result<Option<PathBuf>, MigrationError> {
    if !path.exists() {
        return Ok(None);
    }
    let version = file_version(path)?;
    let latest = MIGRATIONS.last().map_or(0, |m| m.version);
    if version > latest {
        return Err(MigrationError::TooNew { version });
    }
    if version == latest {
        return Ok(None);
    }
    let backup = with_suffix(path, &format!(".bak-v{version}"));
    fs::copy(path, &backup)?;
    let wal = with_suffix(path, ".wal");
    if wal.exists() {
        fs::copy(&wal, with_suffix(&backup, ".wal"))?;
    }
    Ok(Some(backup))
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

/// Applies every pending migration in the given transaction and returns the versions applied.
pub fn run(tx: &Transaction<'_>) -> Result<Vec<i32>, MigrationError> {
    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (version INTEGER PRIMARY KEY);
         ALTER TABLE schema_migrations ADD COLUMN IF NOT EXISTS name VARCHAR;
         ALTER TABLE schema_migrations ADD COLUMN IF NOT EXISTS checksum VARCHAR;
         ALTER TABLE schema_migrations ADD COLUMN IF NOT EXISTS applied_at TIMESTAMP;",
    )?;
    let recorded: Vec<(i32, Option<String>)> = {
        let mut stmt = tx.prepare("SELECT version, checksum FROM schema_migrations")?;
        stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<duckdb::Result<_>>()?
    };
    let latest = MIGRATIONS.last().map_or(0, |m| m.version);
    if let Some((version, _)) = recorded.iter().find(|(version, _)| *version > latest) {
        return Err(MigrationError::TooNew { version: *version });
    }

    let mut applied = Vec::new();
    for migration in MIGRATIONS {
        let checksum = migration.checksum();
        match recorded.iter().find(|(version, _)| *version == migration.version) {
            Some((_, Some(stored))) if *stored != checksum => {
                return Err(MigrationError::Changed {
                    version: migration.version,
                });
            }
            Some((_, Some(_))) => {}
            // Recorded before checksums existed: adopt this build's checksum.
            Some((_, None)) => {
                tx.execute(
                    "UPDATE schema_migrations SET name = ?, checksum = ? WHERE version = ?",
                    duckdb::params![migration.name, checksum, migration.version],
                )?;
            }
            None => {
                for part in migration.parts {
                    tx.execute_batch(part)?;
                }
                tx.execute(
                    "INSERT INTO schema_migrations (version, name, checksum, applied_at)
                     VALUES (?, ?, ?, CURRENT_TIMESTAMP)",
                    duckdb::params![migration.version, migration.name, checksum],
                )?;
                applied.push(migration.version);
            }
        }
    }
    Ok(applied)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ot-migrate-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir.join("book.duckdb")
    }

    fn migrate(connection: &mut Connection) -> Result<Vec<i32>, MigrationError> {
        let tx = connection.transaction()?;
        let applied = run(&tx)?;
        tx.commit()?;
        Ok(applied)
    }

    #[test]
    fn versions_are_increasing_and_checksums_stable() {
        let versions: Vec<i32> = MIGRATIONS.iter().map(|m| m.version).collect();
        assert!(versions.windows(2).all(|w| w[0] < w[1]));
        assert_eq!(versions.first(), Some(&1));
        assert_eq!(MIGRATIONS[0].checksum(), MIGRATIONS[0].checksum());
    }

    #[test]
    fn fresh_book_applies_all_then_nothing() {
        let mut connection = Connection::open_in_memory().unwrap();
        assert_eq!(migrate(&mut connection).unwrap(), vec![1]);
        assert!(migrate(&mut connection).unwrap().is_empty());
        let (name, checksum): (String, String) = connection
            .query_row(
                "SELECT name, checksum FROM schema_migrations WHERE version = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(name, "initial");
        assert_eq!(checksum, MIGRATIONS[0].checksum());
    }

    #[test]
    fn pre_runner_book_adopts_checksum() {
        let mut connection = Connection::open_in_memory().unwrap();
        migrate(&mut connection).unwrap();
        connection
            .execute_batch("UPDATE schema_migrations SET name = NULL, checksum = NULL")
            .unwrap();
        assert!(migrate(&mut connection).unwrap().is_empty());
        let checksum: String = connection
            .query_row("SELECT checksum FROM schema_migrations", [], |row| row.get(0))
            .unwrap();
        assert_eq!(checksum, MIGRATIONS[0].checksum());
    }

    #[test]
    fn changed_migration_is_refused() {
        let mut connection = Connection::open_in_memory().unwrap();
        migrate(&mut connection).unwrap();
        connection
            .execute_batch("UPDATE schema_migrations SET checksum = 'edited'")
            .unwrap();
        assert!(matches!(
            migrate(&mut connection),
            Err(MigrationError::Changed { version: 1 })
        ));
    }

    #[test]
    fn newer_book_is_refused() {
        let mut connection = Connection::open_in_memory().unwrap();
        migrate(&mut connection).unwrap();
        connection
            .execute_batch("INSERT INTO schema_migrations (version) VALUES (9999)")
            .unwrap();
        assert!(matches!(
            migrate(&mut connection),
            Err(MigrationError::TooNew { version: 9999 })
        ));
    }

    #[test]
    fn backup_is_made_only_when_migrations_are_pending() {
        let path = temp_path("backup");
        assert_eq!(backup_before_migrating(&path).unwrap(), None);

        // A Voucher Desk book: no schema_migrations table yet.
        Connection::open(&path)
            .unwrap()
            .execute_batch(include_str!("../migrations/duckdb/0001_initial/1_legacy.sql"))
            .unwrap();
        assert_eq!(file_version(&path).unwrap(), 0);
        let backup = backup_before_migrating(&path).unwrap().unwrap();
        assert!(backup.to_string_lossy().ends_with(".bak-v0"));
        assert!(backup.exists());

        let mut connection = Connection::open(&path).unwrap();
        migrate(&mut connection).unwrap();
        drop(connection);
        assert_eq!(file_version(&path).unwrap(), 1);
        assert_eq!(backup_before_migrating(&path).unwrap(), None);
    }
}
