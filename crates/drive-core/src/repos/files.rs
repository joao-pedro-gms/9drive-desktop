use rusqlite::{Connection, Row, params};

use crate::db::now_unix;
use crate::error::{AppError, Result};
use crate::models::FileItem;

const COLUMNS: &str = "id, account_id, drive_file_id, folder_id, name, mime_type, size_bytes, \
                       md5_checksum, starred, archived, trashed_at, created_at, updated_at";

fn map(row: &Row<'_>) -> rusqlite::Result<FileItem> {
    Ok(FileItem {
        id: row.get("id")?,
        account_id: row.get("account_id")?,
        drive_file_id: row.get("drive_file_id")?,
        folder_id: row.get("folder_id")?,
        name: row.get("name")?,
        mime_type: row.get("mime_type")?,
        size_bytes: row.get("size_bytes")?,
        md5_checksum: row.get("md5_checksum")?,
        starred: row.get::<_, i64>("starred")? != 0,
        archived: row.get::<_, i64>("archived")? != 0,
        trashed_at: row.get("trashed_at")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}

/// `id`, `created_at` and `updated_at` on `file` are ignored; the row is stamped here.
pub fn insert(conn: &Connection, file: &FileItem) -> Result<FileItem> {
    let now = now_unix();
    conn.execute(
        "INSERT INTO files (
             account_id, drive_file_id, folder_id, name, mime_type, size_bytes, md5_checksum,
             starred, archived, created_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10)",
        params![
            file.account_id,
            file.drive_file_id,
            file.folder_id,
            file.name,
            file.mime_type,
            file.size_bytes,
            file.md5_checksum,
            file.starred as i64,
            file.archived as i64,
            now
        ],
    )?;
    get(conn, conn.last_insert_rowid())
}

/// Trashed rows are hidden; `None` lists files that belong to no folder.
pub fn list_by_folder(conn: &Connection, folder_id: Option<i64>) -> Result<Vec<FileItem>> {
    let sql = format!(
        "SELECT {COLUMNS} FROM files WHERE folder_id IS ?1 AND trashed_at IS NULL ORDER BY name, id"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([folder_id], map)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

pub fn get(conn: &Connection, id: i64) -> Result<FileItem> {
    let sql = format!("SELECT {COLUMNS} FROM files WHERE id = ?1");
    match conn.query_row(&sql, [id], map) {
        Ok(file) => Ok(file),
        Err(rusqlite::Error::QueryReturnedNoRows) => Err(AppError::not_found("file", id)),
        Err(other) => Err(AppError::Database(other)),
    }
}

pub fn set_starred(conn: &Connection, id: i64, starred: bool) -> Result<()> {
    update_flag(conn, id, "starred", starred as i64)
}

pub fn set_archived(conn: &Connection, id: i64, archived: bool) -> Result<()> {
    update_flag(conn, id, "archived", archived as i64)
}

pub fn trash(conn: &Connection, id: i64) -> Result<()> {
    set_trashed_at(conn, id, Some(now_unix()))
}

pub fn untrash(conn: &Connection, id: i64) -> Result<()> {
    set_trashed_at(conn, id, None)
}

pub fn rename(conn: &Connection, id: i64, name: &str) -> Result<()> {
    if name.trim().is_empty() {
        return Err(AppError::InvalidInput("file name must not be blank".into()));
    }
    update(conn, id, "name = ?2", params![id, name, now_unix()])
}

pub fn move_to_folder(conn: &Connection, id: i64, folder_id: Option<i64>) -> Result<()> {
    update(conn, id, "folder_id = ?2", params![id, folder_id, now_unix()])
}

pub fn delete(conn: &Connection, id: i64) -> Result<()> {
    let changed = conn.execute("DELETE FROM files WHERE id = ?1", [id])?;
    if changed == 0 {
        return Err(AppError::not_found("file", id));
    }
    Ok(())
}

/// SQLite `LIKE` folds case for ASCII only; F4 replaces this with the provider query.
pub fn search_by_name(conn: &Connection, query: &str, limit: i64) -> Result<Vec<FileItem>> {
    let sql = format!(
        "SELECT {COLUMNS} FROM files
         WHERE name LIKE ?1 ESCAPE '\\' AND trashed_at IS NULL
         ORDER BY name, id LIMIT ?2"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![like_pattern(query), limit], map)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// `%` and `_` typed by the user must match literally, not as wildcards.
fn like_pattern(query: &str) -> String {
    let mut pattern = String::with_capacity(query.len() + 2);
    pattern.push('%');
    for ch in query.chars() {
        if matches!(ch, '%' | '_' | '\\') {
            pattern.push('\\');
        }
        pattern.push(ch);
    }
    pattern.push('%');
    pattern
}

fn set_trashed_at(conn: &Connection, id: i64, at: Option<i64>) -> Result<()> {
    update(conn, id, "trashed_at = ?2", params![id, at, now_unix()])
}

fn update_flag(conn: &Connection, id: i64, column: &str, value: i64) -> Result<()> {
    update(conn, id, &format!("{column} = ?2"), params![id, value, now_unix()])
}

/// `args` carries `?1` = id, `?2` = the assigned value and `?3` = updated_at.
fn update(conn: &Connection, id: i64, assignment: &str, args: impl rusqlite::Params) -> Result<()> {
    let sql = format!("UPDATE files SET {assignment}, updated_at = ?3 WHERE id = ?1");
    let changed = conn.execute(&sql, args)?;
    if changed == 0 {
        return Err(AppError::not_found("file", id));
    }
    Ok(())
}

#[cfg(test)]
pub(crate) fn sample(account_id: i64, name: &str, folder_id: Option<i64>) -> FileItem {
    FileItem {
        id: 0,
        account_id,
        drive_file_id: format!("drive-{name}"),
        folder_id,
        name: name.to_string(),
        mime_type: Some("text/plain".into()),
        size_bytes: Some(12),
        md5_checksum: None,
        starred: false,
        archived: false,
        trashed_at: None,
        created_at: 0,
        updated_at: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_conn;
    use crate::repos::accounts;
    use crate::repos::folders;

    fn setup() -> (Connection, i64) {
        let conn = test_conn();
        let account = accounts::create(&conn, "google", "User", "a@example.com").unwrap();
        (conn, account.id)
    }

    #[test]
    fn insert_then_get_keeps_every_column() {
        let (conn, account_id) = setup();
        let folder = folders::create(&conn, "Docs", None).unwrap();
        let mut file = sample(account_id, "notes.txt", Some(folder.id));
        file.md5_checksum = Some("deadbeef".into());
        file.starred = true;

        let inserted = insert(&conn, &file).unwrap();
        assert!(inserted.id > 0);
        assert_eq!(inserted.created_at, inserted.updated_at);
        assert!(inserted.starred);
        assert_eq!(inserted, get(&conn, inserted.id).unwrap());
        assert_eq!(inserted.md5_checksum.as_deref(), Some("deadbeef"));
    }

    #[test]
    fn duplicate_drive_file_id_is_an_error_not_a_panic() {
        let (conn, account_id) = setup();
        insert(&conn, &sample(account_id, "dup.txt", None)).unwrap();
        let err = insert(&conn, &sample(account_id, "dup.txt", None)).unwrap_err();
        assert!(matches!(err, AppError::Database(_)), "got {err:?}");
        assert_eq!(err.code(), "database_error");
    }

    #[test]
    fn list_by_folder_scopes_to_the_folder_and_hides_trashed() {
        let (conn, account_id) = setup();
        let docs = folders::create(&conn, "Docs", None).unwrap();
        let pics = folders::create(&conn, "Pics", None).unwrap();
        let in_docs = insert(&conn, &sample(account_id, "a.txt", Some(docs.id))).unwrap();
        insert(&conn, &sample(account_id, "b.txt", Some(pics.id))).unwrap();
        let loose = insert(&conn, &sample(account_id, "loose.txt", None)).unwrap();

        let listed: Vec<i64> =
            list_by_folder(&conn, Some(docs.id)).unwrap().into_iter().map(|f| f.id).collect();
        assert_eq!(listed, [in_docs.id]);

        trash(&conn, in_docs.id).unwrap();
        assert!(list_by_folder(&conn, Some(docs.id)).unwrap().is_empty());
        assert_eq!(list_by_folder(&conn, None).unwrap().len(), 1);
        assert_eq!(list_by_folder(&conn, None).unwrap()[0].id, loose.id);
        let pics_names: Vec<String> =
            list_by_folder(&conn, Some(pics.id)).unwrap().into_iter().map(|f| f.name).collect();
        assert_eq!(pics_names, ["b.txt"]);
    }

    #[test]
    fn trash_then_untrash_round_trips() {
        let (conn, account_id) = setup();
        let file = insert(&conn, &sample(account_id, "gone.txt", None)).unwrap();

        trash(&conn, file.id).unwrap();
        let trashed = get(&conn, file.id).unwrap();
        assert!(trashed.trashed_at.is_some());

        untrash(&conn, file.id).unwrap();
        assert_eq!(get(&conn, file.id).unwrap().trashed_at, None);

        assert!(matches!(trash(&conn, 999), Err(AppError::NotFound { .. })));
    }

    #[test]
    fn flags_and_rename_and_move_update_the_row() {
        let (conn, account_id) = setup();
        let src = folders::create(&conn, "Src", None).unwrap();
        let dst = folders::create(&conn, "Dst", None).unwrap();
        let file = insert(&conn, &sample(account_id, "old.txt", Some(src.id))).unwrap();

        set_starred(&conn, file.id, true).unwrap();
        set_archived(&conn, file.id, true).unwrap();
        assert!(get(&conn, file.id).unwrap().starred);
        assert!(get(&conn, file.id).unwrap().archived);
        set_starred(&conn, file.id, false).unwrap();
        assert!(!get(&conn, file.id).unwrap().starred);

        rename(&conn, file.id, "new.txt").unwrap();
        assert_eq!(get(&conn, file.id).unwrap().name, "new.txt");
        assert!(matches!(rename(&conn, file.id, " "), Err(AppError::InvalidInput(_))));

        move_to_folder(&conn, file.id, Some(dst.id)).unwrap();
        assert_eq!(get(&conn, file.id).unwrap().folder_id, Some(dst.id));
        assert!(list_by_folder(&conn, Some(src.id)).unwrap().is_empty());

        assert!(matches!(delete(&conn, 999), Err(AppError::NotFound { .. })));
        delete(&conn, file.id).unwrap();
        assert!(matches!(get(&conn, file.id), Err(AppError::NotFound { .. })));
    }

    #[test]
    fn search_by_name_is_case_insensitive_and_limited() {
        let (conn, account_id) = setup();
        insert(&conn, &sample(account_id, "Report.PDF", None)).unwrap();
        insert(&conn, &sample(account_id, "report_final.pdf", None)).unwrap();
        insert(&conn, &sample(account_id, "notes.txt", None)).unwrap();
        let hidden = insert(&conn, &sample(account_id, "report_old.pdf", None)).unwrap();
        trash(&conn, hidden.id).unwrap();

        let hits: Vec<String> =
            search_by_name(&conn, "REPORT", 10).unwrap().into_iter().map(|f| f.name).collect();
        assert_eq!(hits.len(), 2, "trashed rows must stay out of search: {hits:?}");
        assert!(hits.contains(&"Report.PDF".to_string()));

        assert_eq!(search_by_name(&conn, "report", 1).unwrap().len(), 1);
        assert!(search_by_name(&conn, "nothing-matches", 10).unwrap().is_empty());
    }

    #[test]
    fn search_treats_wildcards_as_literal_text() {
        let (conn, account_id) = setup();
        insert(&conn, &sample(account_id, "100% done.txt", None)).unwrap();
        insert(&conn, &sample(account_id, "plain.txt", None)).unwrap();

        assert_eq!(search_by_name(&conn, "%", 10).unwrap().len(), 1);
        assert_eq!(search_by_name(&conn, "100%", 10).unwrap().len(), 1);
        assert_eq!(search_by_name(&conn, "p_a", 10).unwrap().len(), 0);
    }

    #[test]
    fn deleting_the_account_cascades_to_files() {
        let (conn, account_id) = setup();
        let file = insert(&conn, &sample(account_id, "doomed.txt", None)).unwrap();
        accounts::delete(&conn, account_id).unwrap();
        assert!(matches!(get(&conn, file.id), Err(AppError::NotFound { .. })));
    }
}
