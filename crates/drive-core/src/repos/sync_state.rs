use rusqlite::{Connection, Row, params};

use crate::db::now_unix;
use crate::error::{AppError, Result};
use crate::models::SyncState;

const COLUMNS: &str = "account_id, start_page_token, last_sync_at";

fn map(row: &Row<'_>) -> rusqlite::Result<SyncState> {
    Ok(SyncState {
        account_id: row.get("account_id")?,
        start_page_token: row.get("start_page_token")?,
        last_sync_at: row.get("last_sync_at")?,
    })
}

pub fn get(conn: &Connection, account_id: i64) -> Result<SyncState> {
    let sql = format!("SELECT {COLUMNS} FROM sync_state WHERE account_id = ?1");
    match conn.query_row(&sql, [account_id], map) {
        Ok(state) => Ok(state),
        Err(rusqlite::Error::QueryReturnedNoRows) => {
            Err(AppError::not_found("sync state", account_id))
        }
        Err(other) => Err(AppError::Database(other)),
    }
}

/// `None` clears the page token, which is how a full re-sync starts over.
pub fn upsert(conn: &Connection, account_id: i64, page_token: Option<&str>) -> Result<()> {
    conn.execute(
        "INSERT INTO sync_state (account_id, start_page_token, last_sync_at) VALUES (?1, ?2, ?3)
         ON CONFLICT(account_id) DO UPDATE SET
             start_page_token = excluded.start_page_token,
             last_sync_at = excluded.last_sync_at",
        params![account_id, page_token, now_unix()],
    )?;
    Ok(())
}

pub fn list_all(conn: &Connection) -> Result<Vec<SyncState>> {
    let sql = format!("SELECT {COLUMNS} FROM sync_state ORDER BY account_id");
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], map)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_conn;
    use crate::repos::accounts;

    fn setup() -> (Connection, i64) {
        let conn = test_conn();
        let account = accounts::create(&conn, "google", "User", "a@example.com").unwrap();
        (conn, account.id)
    }

    #[test]
    fn get_missing_state_is_not_found() {
        let conn = test_conn();
        assert!(matches!(get(&conn, 1), Err(AppError::NotFound { .. })));
    }

    #[test]
    fn upsert_round_trips_the_page_token() {
        let (conn, account_id) = setup();
        upsert(&conn, account_id, Some("token-1")).unwrap();

        let state = get(&conn, account_id).unwrap();
        assert_eq!(state.account_id, account_id);
        assert_eq!(state.start_page_token.as_deref(), Some("token-1"));
        assert!(state.last_sync_at.is_some_and(|at| at > 0));

        upsert(&conn, account_id, Some("token-2")).unwrap();
        assert_eq!(get(&conn, account_id).unwrap().start_page_token.as_deref(), Some("token-2"));
        assert_eq!(list_all(&conn).unwrap().len(), 1);
    }

    #[test]
    fn upsert_with_none_clears_the_page_token() {
        let (conn, account_id) = setup();
        upsert(&conn, account_id, Some("token-1")).unwrap();
        upsert(&conn, account_id, None).unwrap();

        let state = get(&conn, account_id).unwrap();
        assert_eq!(state.start_page_token, None);
        assert!(state.last_sync_at.is_some());
    }

    #[test]
    fn upsert_requires_an_existing_account() {
        let conn = test_conn();
        assert!(matches!(upsert(&conn, 404, Some("t")), Err(AppError::Database(_))));
    }
}
