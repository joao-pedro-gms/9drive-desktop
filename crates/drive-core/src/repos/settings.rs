use rusqlite::{Connection, OptionalExtension, params};

use crate::error::Result;

pub fn get(conn: &Connection, key: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row("SELECT value FROM settings WHERE key = ?1", [key], |row| row.get(0))
        .optional()?)
}

pub fn set(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

pub fn all(conn: &Connection) -> Result<Vec<(String, String)>> {
    let mut stmt = conn.prepare("SELECT key, value FROM settings ORDER BY key")?;
    let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

pub fn delete(conn: &Connection, key: &str) -> Result<()> {
    conn.execute("DELETE FROM settings WHERE key = ?1", [key])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_conn;

    #[test]
    fn missing_key_is_none_not_an_error() {
        let conn = test_conn();
        assert_eq!(get(&conn, "theme").unwrap(), None);
        assert!(all(&conn).unwrap().is_empty());
    }

    #[test]
    fn set_replaces_an_existing_value() {
        let conn = test_conn();
        set(&conn, "theme", "dark").unwrap();
        set(&conn, "theme", "light").unwrap();
        assert_eq!(get(&conn, "theme").unwrap().as_deref(), Some("light"));
        assert_eq!(all(&conn).unwrap().len(), 1);
    }

    #[test]
    fn all_is_sorted_by_key() {
        let conn = test_conn();
        set(&conn, "theme", "dark").unwrap();
        set(&conn, "google_client_id", "abc.apps.googleusercontent.com").unwrap();
        set(&conn, "language", "pt-BR").unwrap();

        let keys: Vec<String> = all(&conn).unwrap().into_iter().map(|(k, _)| k).collect();
        assert_eq!(keys, ["google_client_id", "language", "theme"]);
    }

    #[test]
    fn delete_removes_the_key_and_is_idempotent() {
        let conn = test_conn();
        set(&conn, "theme", "dark").unwrap();
        delete(&conn, "theme").unwrap();
        delete(&conn, "theme").unwrap();
        assert_eq!(get(&conn, "theme").unwrap(), None);
        assert!(all(&conn).unwrap().is_empty());
    }
}
