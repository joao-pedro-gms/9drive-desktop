use rusqlite::{Connection, Row, params};

use crate::db::now_unix;
use crate::error::{AppError, Result};
use crate::models::{Account, AccountQuota};

const COLUMNS: &str =
    "id, provider, name, email, external_account_id, drive_folder_id, created_at, last_used_at";

fn map(row: &Row<'_>) -> rusqlite::Result<Account> {
    Ok(Account {
        id: row.get("id")?,
        provider: row.get("provider")?,
        name: row.get("name")?,
        email: row.get("email")?,
        external_account_id: row.get("external_account_id")?,
        drive_folder_id: row.get("drive_folder_id")?,
        created_at: row.get("created_at")?,
        last_used_at: row.get("last_used_at")?,
    })
}

pub fn create(conn: &Connection, provider: &str, name: &str, email: &str) -> Result<Account> {
    conn.execute(
        "INSERT INTO accounts (provider, name, email, created_at) VALUES (?1, ?2, ?3, ?4)",
        params![provider, name, email, now_unix()],
    )?;
    get(conn, conn.last_insert_rowid())
}

pub fn list(conn: &Connection) -> Result<Vec<Account>> {
    let sql = format!("SELECT {COLUMNS} FROM accounts ORDER BY created_at, id");
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], map)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

pub fn get(conn: &Connection, id: i64) -> Result<Account> {
    let sql = format!("SELECT {COLUMNS} FROM accounts WHERE id = ?1");
    conn.query_row(&sql, [id], map).map_err(|e| not_found_or(e, "account", id))
}

/// `last_used_at` is set on every write; pass `None` to clear it.
pub fn update(conn: &Connection, id: i64, name: &str, last_used_at: Option<i64>) -> Result<()> {
    let changed = conn.execute(
        "UPDATE accounts SET name = ?2, last_used_at = ?3 WHERE id = ?1",
        params![id, name, last_used_at],
    )?;
    require_changed(changed, "account", id)
}

/// Cascades to `account_quota`, `sync_state` and `files` (ON DELETE CASCADE).
pub fn delete(conn: &Connection, id: i64) -> Result<()> {
    let changed = conn.execute("DELETE FROM accounts WHERE id = ?1", [id])?;
    require_changed(changed, "account", id)
}

/// `synced_at` is always stamped here, so the input value is ignored.
pub fn upsert_quota(conn: &Connection, quota: &AccountQuota) -> Result<()> {
    conn.execute(
        "INSERT INTO account_quota (
             account_id, limit_bytes, usage_bytes, usage_in_drive, usage_in_drive_trash, synced_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(account_id) DO UPDATE SET
             limit_bytes = excluded.limit_bytes,
             usage_bytes = excluded.usage_bytes,
             usage_in_drive = excluded.usage_in_drive,
             usage_in_drive_trash = excluded.usage_in_drive_trash,
             synced_at = excluded.synced_at",
        params![
            quota.account_id,
            quota.limit_bytes,
            quota.usage_bytes,
            quota.usage_in_drive,
            quota.usage_in_drive_trash,
            now_unix()
        ],
    )?;
    Ok(())
}

pub fn get_quota(conn: &Connection, account_id: i64) -> Result<AccountQuota> {
    conn.query_row(
        "SELECT account_id, limit_bytes, usage_bytes, usage_in_drive, usage_in_drive_trash, synced_at
         FROM account_quota WHERE account_id = ?1",
        [account_id],
        |row| {
            Ok(AccountQuota {
                account_id: row.get("account_id")?,
                limit_bytes: row.get("limit_bytes")?,
                usage_bytes: row.get("usage_bytes")?,
                usage_in_drive: row.get("usage_in_drive")?,
                usage_in_drive_trash: row.get("usage_in_drive_trash")?,
                synced_at: row.get("synced_at")?,
            })
        },
    )
    .map_err(|e| not_found_or(e, "account quota", account_id))
}

pub fn delete_quota(conn: &Connection, account_id: i64) -> Result<()> {
    let changed = conn.execute("DELETE FROM account_quota WHERE account_id = ?1", [account_id])?;
    require_changed(changed, "account quota", account_id)
}

fn require_changed(changed: usize, entity: &'static str, id: i64) -> Result<()> {
    if changed == 0 {
        return Err(AppError::not_found(entity, id));
    }
    Ok(())
}

/// Turns "no such row" into `NotFound` while leaving real SQL errors alone.
fn not_found_or(err: rusqlite::Error, entity: &'static str, id: i64) -> AppError {
    match err {
        rusqlite::Error::QueryReturnedNoRows => AppError::not_found(entity, id),
        other => AppError::Database(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_conn;
    use crate::repos::sync_state;

    fn new_account(conn: &Connection, email: &str) -> Account {
        create(conn, "google", "Test User", email).unwrap()
    }

    fn quota(account_id: i64, usage: i64) -> AccountQuota {
        AccountQuota {
            account_id,
            limit_bytes: 15_000_000_000_000,
            usage_bytes: usage,
            usage_in_drive: Some(usage),
            usage_in_drive_trash: Some(0),
            synced_at: 0,
        }
    }

    #[test]
    fn create_then_get_and_list() {
        let conn = test_conn();
        let account = new_account(&conn, "a@example.com");
        assert!(account.id > 0);
        assert_eq!(account.provider, "google");
        assert!(account.created_at > 0);
        assert_eq!(account.external_account_id, None);

        assert_eq!(get(&conn, account.id).unwrap(), account);
        new_account(&conn, "b@example.com");
        let all = list(&conn).unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].email, "a@example.com");
    }

    #[test]
    fn get_missing_account_is_not_found() {
        let conn = test_conn();
        assert!(matches!(get(&conn, 404), Err(AppError::NotFound { entity: "account", id: 404 })));
    }

    #[test]
    fn update_changes_name_and_last_used_at() {
        let conn = test_conn();
        let account = new_account(&conn, "a@example.com");

        update(&conn, account.id, "Renamed", Some(1_700_000_000)).unwrap();
        let updated = get(&conn, account.id).unwrap();
        assert_eq!(updated.name, "Renamed");
        assert_eq!(updated.last_used_at, Some(1_700_000_000));
        assert_eq!(updated.email, "a@example.com");

        assert!(matches!(update(&conn, 999, "x", None), Err(AppError::NotFound { .. })));
    }

    #[test]
    fn upsert_quota_overwrites_instead_of_duplicating() {
        let conn = test_conn();
        let account = new_account(&conn, "a@example.com");

        upsert_quota(&conn, &quota(account.id, 100)).unwrap();
        let first = get_quota(&conn, account.id).unwrap();
        upsert_quota(&conn, &quota(account.id, 250)).unwrap();

        let stored = get_quota(&conn, account.id).unwrap();
        assert_eq!(stored.account_id, account.id);
        assert_eq!(stored.usage_bytes, 250);
        assert_eq!(stored.limit_bytes, 15_000_000_000_000);
        assert_eq!(stored.usage_in_drive, Some(250));
        assert!(stored.synced_at >= first.synced_at);

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM account_quota WHERE account_id = ?1",
                [account.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn delete_cascades_to_quota_and_sync_state() {
        let conn = test_conn();
        let account = new_account(&conn, "a@example.com");
        upsert_quota(&conn, &quota(account.id, 100)).unwrap();
        sync_state::upsert(&conn, account.id, Some("token-1")).unwrap();

        delete(&conn, account.id).unwrap();

        assert!(matches!(get(&conn, account.id), Err(AppError::NotFound { .. })));
        assert!(matches!(get_quota(&conn, account.id), Err(AppError::NotFound { .. })));
        assert!(matches!(sync_state::get(&conn, account.id), Err(AppError::NotFound { .. })));
        assert!(matches!(delete(&conn, account.id), Err(AppError::NotFound { .. })));
    }

    #[test]
    fn delete_quota_is_reported_when_missing() {
        let conn = test_conn();
        assert!(matches!(delete_quota(&conn, 7), Err(AppError::NotFound { .. })));
    }
}
