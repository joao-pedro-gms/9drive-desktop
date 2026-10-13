//! SQLite connection setup and the migration runner.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::Connection;

use crate::error::Result;

/// Every file in `migrations/`, applied in name order. Never edit an applied migration:
/// add a new numbered file instead.
const MIGRATIONS: &[(&str, &str)] =
    &[("0001_initial", include_str!("../migrations/0001_initial.sql"))];

/// Unix epoch **seconds**, and booleans as `INTEGER` 0/1: this crate ships no date
/// dependency and SQLite has no boolean type, so both stay plain integers.
pub(crate) fn now_unix() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

pub fn open(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path)?;
    configure(&conn)?;
    migrate(&conn)?;
    Ok(conn)
}

pub fn open_in_memory() -> Result<Connection> {
    let conn = Connection::open_in_memory()?;
    configure(&conn)?;
    migrate(&conn)?;
    Ok(conn)
}

fn configure(conn: &Connection) -> Result<()> {
    // WAL lets the UI read while the sync engine writes; foreign_keys is off by default
    // in SQLite and the cascade rules in the schema depend on it.
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
    Ok(())
}

/// Applies pending migrations, returning how many ran.
pub fn migrate(conn: &Connection) -> Result<usize> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS _migrations (
             name      TEXT PRIMARY KEY,
             applied_at INTEGER NOT NULL
         )",
    )?;

    let mut applied = 0;
    for (name, sql) in MIGRATIONS.iter().copied() {
        let seen: i64 =
            conn.query_row("SELECT COUNT(*) FROM _migrations WHERE name = ?1", [name], |row| {
                row.get(0)
            })?;
        if seen > 0 {
            continue;
        }
        // One transaction per file: a failed migration must not be recorded as applied.
        let tx = conn.unchecked_transaction()?;
        tx.execute_batch(sql)?;
        tx.execute(
            "INSERT INTO _migrations (name, applied_at) VALUES (?1, ?2)",
            rusqlite::params![name, now_unix()],
        )?;
        tx.commit()?;
        applied += 1;
    }
    Ok(applied)
}

#[cfg(test)]
pub(crate) fn test_conn() -> Connection {
    open_in_memory().expect("in-memory database with migrations applied")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_in_memory_applies_every_migration_and_records_it() {
        let conn = open_in_memory().unwrap();
        let names: Vec<String> = conn
            .prepare("SELECT name FROM _migrations ORDER BY name")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();

        let expected: Vec<String> =
            MIGRATIONS.iter().map(|(name, _)| (*name).to_string()).collect();
        assert_eq!(names, expected);
        assert!(!names.is_empty(), "the crate needs at least one migration");
    }

    #[test]
    fn migrate_is_idempotent() {
        let conn = test_conn();
        assert_eq!(migrate(&conn).unwrap(), 0);
        assert_eq!(migrate(&conn).unwrap(), 0);

        let recorded: i64 =
            conn.query_row("SELECT COUNT(*) FROM _migrations", [], |row| row.get(0)).unwrap();
        assert_eq!(recorded as usize, MIGRATIONS.len());
    }

    #[test]
    fn configure_enables_foreign_keys() {
        let conn = test_conn();
        let enabled: i64 = conn
            .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
            .expect("pragma is readable");
        assert_eq!(enabled, 1);
    }

    #[test]
    fn now_unix_is_seconds_since_epoch() {
        let now = now_unix();
        assert!(now > 1_600_000_000, "seconds, not milliseconds: {now}");
        assert!(now < 100_000_000_000);
    }

    #[test]
    fn schema_matches_the_fixed_shape() {
        let conn = test_conn();
        for table in ["settings", "accounts", "account_quota", "folders", "files", "sync_state"] {
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                    [table],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(count, 1, "table {table} is missing");
        }
        for index in [
            "idx_files_account_id",
            "idx_files_folder_id",
            "idx_files_name",
            "idx_folders_parent_id",
        ] {
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = ?1",
                    [index],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(count, 1, "index {index} is missing");
        }
    }
}
