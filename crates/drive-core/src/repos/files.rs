use drive_storage::types::RemoteFile;
use rusqlite::{Connection, Row, params, types::Value};

use crate::db::now_unix;
use crate::error::{AppError, Result};
use crate::models::FileItem;

const COLUMNS: &str = "id, account_id, drive_file_id, folder_id, name, mime_type, size_bytes, \
                       md5_checksum, starred, archived, trashed_at, created_at, updated_at";

/// How stale a local row must be before a "gone from Drive" change is believed.
const RECENT_WRITE_MARGIN_SECS: i64 = 60;

/// Which folder a listing is scoped to. A distinct type rather than `Option<i64>`, because "no
/// filter", "this folder" and "files in no folder" are three different queries and `Option<i64>`
/// can only say two of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FolderScope {
    #[default]
    All,
    Folder(i64),
    Loose,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FileFilter {
    pub folder: FolderScope,
    pub starred: Option<bool>,
    pub archived: Option<bool>,
    /// `true` lists trashed rows only; the default lists active rows only.
    pub trashed: bool,
    /// Case-insensitive substring on name. Empty means "no search", not "match nothing", so a
    /// search box that the user has cleared keeps listing instead of going blank. The search is
    /// local on purpose: Drive's `9drive` folder is the source of truth, but the local mirror is
    /// what the UI searches, because a remote `q` would miss whatever no sync has landed yet and
    /// would cost a round trip per keystroke.
    pub search: Option<String>,
    pub limit: Option<i64>,
}

/// One query for every listing. `trashed_at` is always constrained, so a caller cannot leak
/// trashed rows into a normal listing by forgetting a field.
pub fn list(conn: &Connection, filter: &FileFilter) -> Result<Vec<FileItem>> {
    let mut sql = format!("SELECT {COLUMNS} FROM files WHERE 1 = 1");
    let mut args: Vec<Value> = Vec::new();

    match filter.folder {
        FolderScope::All => {}
        FolderScope::Folder(id) => {
            sql.push_str(" AND folder_id = ?");
            args.push(id.into());
        }
        FolderScope::Loose => sql.push_str(" AND folder_id IS NULL"),
    }
    for (column, flag) in [("starred", filter.starred), ("archived", filter.archived)] {
        if let Some(flag) = flag {
            sql.push_str(&format!(" AND {column} = ?"));
            args.push((flag as i64).into());
        }
    }
    sql.push_str(if filter.trashed {
        " AND trashed_at IS NOT NULL"
    } else {
        " AND trashed_at IS NULL"
    });

    if let Some(query) = filter.search.as_deref().filter(|query| !query.is_empty()) {
        sql.push_str(" AND name LIKE ?");
        args.push(like_pattern(query).into());
        sql.push_str(" ESCAPE '\\'");
    }
    sql.push_str(" ORDER BY name, id");
    if let Some(limit) = filter.limit {
        sql.push_str(" LIMIT ?");
        args.push(limit.into());
    }

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(args), map)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Trashed rows are hidden; `None` lists files that belong to no folder.
pub fn list_by_folder(conn: &Connection, folder_id: Option<i64>) -> Result<Vec<FileItem>> {
    let folder = match folder_id {
        Some(id) => FolderScope::Folder(id),
        None => FolderScope::Loose,
    };
    list(conn, &FileFilter { folder, ..Default::default() })
}

pub fn get(conn: &Connection, id: i64) -> Result<FileItem> {
    let sql = format!("SELECT {COLUMNS} FROM files WHERE id = ?1");
    match conn.query_row(&sql, [id], map) {
        Ok(file) => Ok(file),
        Err(rusqlite::Error::QueryReturnedNoRows) => Err(AppError::not_found("file", id)),
        Err(other) => Err(AppError::Database(other)),
    }
}

/// Mirrors one `RemoteFile` into the local index, keyed on the `drive_file_id` UNIQUE index. Only
/// the columns Drive owns are written: `starred`, `archived` and `folder_id` are local state that a
/// sync must never clobber.
///
/// shortcut: `folder_id` is left alone because `RemoteFile::parent_id` is a Drive id and cannot be
/// mapped to a local folder row here; F5's folder reconciliation owns that mapping.
pub fn upsert_remote(conn: &Connection, account_id: i64, remote: &RemoteFile) -> Result<FileItem> {
    let now = now_unix();
    let sql = "INSERT INTO files (
                   account_id, drive_file_id, folder_id, name, mime_type, size_bytes, md5_checksum,
                   starred, archived, trashed_at, created_at, updated_at
               ) VALUES (?1, ?2, NULL, ?3, ?4, ?5, ?6, 0, 0, ?7, ?8, ?8)
               ON CONFLICT(drive_file_id) DO UPDATE SET
                   name = excluded.name, mime_type = excluded.mime_type,
                   size_bytes = excluded.size_bytes, md5_checksum = excluded.md5_checksum,
                   trashed_at = excluded.trashed_at, updated_at = ?8
               RETURNING id";
    let id: i64 = conn.query_row(
        sql,
        params![
            account_id,
            &remote.id,
            &remote.name,
            &remote.mime_type,
            remote.size_bytes.map(|bytes| bytes as i64),
            &remote.md5_checksum,
            remote.trashed.then_some(now),
            now
        ],
        |row| row.get(0),
    )?;
    get(conn, id)
}

/// Marks rows missing from Drive as trashed and returns how many changed. A hard delete would be
/// wrong: local rows are a mirror, so "gone" is a flag, not a removal.
///
/// Rows written within [`RECENT_WRITE_MARGIN_SECS`] are skipped. Such a row was almost certainly
/// written by a local action whose provider round trip has not finished, so Drive reporting it gone
/// is lag rather than deletion; leaving it unmarked lets the next run decide.
pub fn mark_deleted(
    conn: &Connection,
    account_id: i64,
    drive_file_ids: &[String],
) -> Result<usize> {
    let now = now_unix();
    let sql = "UPDATE files SET trashed_at = ?1, updated_at = ?1
               WHERE account_id = ?2 AND drive_file_id = ?3 AND trashed_at IS NULL
                 AND updated_at <= ?4";
    let mut stmt = conn.prepare(sql)?;
    let mut marked = 0;
    for drive_file_id in drive_file_ids {
        marked +=
            stmt.execute(params![now, account_id, drive_file_id, now - RECENT_WRITE_MARGIN_SECS])?;
    }
    Ok(marked)
}

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

pub fn set_starred(conn: &Connection, id: i64, starred: bool) -> Result<()> {
    update_flag(conn, id, "starred", starred as i64)
}

pub fn set_archived(conn: &Connection, id: i64, archived: bool) -> Result<()> {
    update_flag(conn, id, "archived", archived as i64)
}

/// Trashes locally only. Callers must push it to Drive and only then write here, or a sync that
/// runs in between will read Drive's pre-trash state through [`upsert_remote`] and untrash the row
/// again. It self-heals on the next run, but the user sees the wrong state until then.
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

    fn remote(id: &str, name: &str) -> RemoteFile {
        RemoteFile {
            id: id.to_string(),
            name: name.to_string(),
            mime_type: "text/plain".into(),
            size_bytes: Some(12),
            md5_checksum: None,
            parent_id: None,
            trashed: false,
            modified_at: None,
            can_download: true,
        }
    }

    fn setup() -> (Connection, i64) {
        let conn = test_conn();
        let account = accounts::create(&conn, "google", "User", "a@example.com").unwrap();
        (conn, account.id)
    }

    fn names(files: Vec<FileItem>) -> Vec<String> {
        files.into_iter().map(|f| f.name).collect()
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
    fn deleting_the_account_cascades_to_files() {
        let (conn, account_id) = setup();
        let file = insert(&conn, &sample(account_id, "doomed.txt", None)).unwrap();
        accounts::delete(&conn, account_id).unwrap();
        assert!(matches!(get(&conn, file.id), Err(AppError::NotFound { .. })));
    }

    #[test]
    fn filters_compose_and_two_narrow_further_than_one_alone() {
        let (conn, account_id) = setup();
        let docs = folders::create(&conn, "Docs", None).unwrap();
        let both = insert(&conn, &sample(account_id, "both.txt", Some(docs.id))).unwrap();
        set_starred(&conn, both.id, true).unwrap();
        set_archived(&conn, both.id, true).unwrap();
        let starred = insert(&conn, &sample(account_id, "starred.txt", Some(docs.id))).unwrap();
        set_starred(&conn, starred.id, true).unwrap();
        let archived = insert(&conn, &sample(account_id, "archived.txt", Some(docs.id))).unwrap();
        set_archived(&conn, archived.id, true).unwrap();
        let plain = insert(&conn, &sample(account_id, "plain.txt", Some(docs.id))).unwrap();
        insert(&conn, &sample(account_id, "elsewhere.txt", None)).unwrap();

        let mut filter = FileFilter { folder: FolderScope::Folder(docs.id), ..Default::default() };
        assert_eq!(
            names(list(&conn, &filter).unwrap()),
            ["archived.txt", "both.txt", "plain.txt", "starred.txt"]
        );

        filter.starred = Some(true);
        assert_eq!(names(list(&conn, &filter).unwrap()), ["both.txt", "starred.txt"]);
        filter.archived = Some(true);
        assert_eq!(
            names(list(&conn, &filter).unwrap()),
            ["both.txt"],
            "starred + archived must narrow past starred alone"
        );
        filter.starred = Some(false);
        assert_eq!(names(list(&conn, &filter).unwrap()), ["archived.txt"]);

        trash(&conn, plain.id).unwrap();
        filter.archived = None;
        filter.starred = None;
        assert_eq!(
            names(list(&conn, &filter).unwrap()),
            ["archived.txt", "both.txt", "starred.txt"],
            "trashed rows stay hidden by default"
        );
        filter.trashed = true;
        assert_eq!(names(list(&conn, &filter).unwrap()), ["plain.txt"]);
    }

    #[test]
    fn default_filter_lists_every_active_row_from_every_folder() {
        let (conn, account_id) = setup();
        let docs = folders::create(&conn, "Docs", None).unwrap();
        insert(&conn, &sample(account_id, "in-folder.txt", Some(docs.id))).unwrap();
        insert(&conn, &sample(account_id, "loose.txt", None)).unwrap();
        let hidden = insert(&conn, &sample(account_id, "hidden.txt", None)).unwrap();
        trash(&conn, hidden.id).unwrap();

        assert_eq!(FileFilter::default().folder, FolderScope::All);
        assert_eq!(
            names(list(&conn, &FileFilter::default()).unwrap()),
            ["in-folder.txt", "loose.txt"]
        );
    }

    #[test]
    fn loose_scope_is_only_rows_with_no_folder_and_differs_from_all() {
        let (conn, account_id) = setup();
        let docs = folders::create(&conn, "Docs", None).unwrap();
        insert(&conn, &sample(account_id, "in-folder.txt", Some(docs.id))).unwrap();
        insert(&conn, &sample(account_id, "loose.txt", None)).unwrap();
        insert(&conn, &sample(account_id, "also-loose.txt", None)).unwrap();

        let loose = FileFilter { folder: FolderScope::Loose, ..Default::default() };
        assert_eq!(names(list(&conn, &loose).unwrap()), ["also-loose.txt", "loose.txt"]);
        assert_eq!(list(&conn, &loose).unwrap().len(), 2);
        assert_eq!(
            list(&conn, &FileFilter::default()).unwrap().len(),
            3,
            "All must reach the folder row too"
        );
    }

    #[test]
    fn search_is_case_insensitive_across_folders_and_honours_the_limit() {
        let (conn, account_id) = setup();
        let docs = folders::create(&conn, "Docs", None).unwrap();
        insert(&conn, &sample(account_id, "Report.PDF", None)).unwrap();
        insert(&conn, &sample(account_id, "report_final.pdf", Some(docs.id))).unwrap();
        insert(&conn, &sample(account_id, "notes.txt", Some(docs.id))).unwrap();
        let hidden = insert(&conn, &sample(account_id, "report_old.pdf", None)).unwrap();
        trash(&conn, hidden.id).unwrap();

        let filter = |query: &str, limit| FileFilter {
            search: Some(query.into()),
            limit,
            ..Default::default()
        };

        let hits = names(list(&conn, &filter("REPORT", Some(10))).unwrap());
        assert_eq!(hits.len(), 2, "trashed rows must stay out of search: {hits:?}");
        assert_eq!(hits, ["Report.PDF", "report_final.pdf"], "search spans every folder: {hits:?}");
        assert_eq!(list(&conn, &filter("report", Some(1))).unwrap().len(), 1);
        assert!(list(&conn, &filter("nothing-matches", Some(10))).unwrap().is_empty());
    }

    #[test]
    fn an_empty_search_is_no_filter_rather_than_a_term() {
        let (conn, account_id) = setup();
        insert(&conn, &sample(account_id, "a.txt", None)).unwrap();
        insert(&conn, &sample(account_id, "b.txt", None)).unwrap();

        let filter = FileFilter { search: Some(String::new()), ..Default::default() };
        assert_eq!(names(list(&conn, &filter).unwrap()), ["a.txt", "b.txt"]);
    }

    #[test]
    fn search_treats_wildcards_as_literal_text() {
        let (conn, account_id) = setup();
        insert(&conn, &sample(account_id, "100% done.txt", None)).unwrap();
        insert(&conn, &sample(account_id, "plain.txt", None)).unwrap();

        let filter = |query: &str| FileFilter { search: Some(query.into()), ..Default::default() };

        assert_eq!(names(list(&conn, &filter("%")).unwrap()), ["100% done.txt"]);
        assert_eq!(names(list(&conn, &filter("100%")).unwrap()), ["100% done.txt"]);
        assert!(list(&conn, &filter("p_a")).unwrap().is_empty());
    }

    #[test]
    fn upsert_remote_inserts_then_refreshes_the_mirror_without_touching_local_state() {
        let (conn, account_id) = setup();
        let docs = folders::create(&conn, "Docs", None).unwrap();

        let first = upsert_remote(&conn, account_id, &remote("drive-1", "before.txt")).unwrap();
        assert!(first.id > 0);
        assert_eq!(first.account_id, account_id);
        assert_eq!(first.drive_file_id, "drive-1");
        assert_eq!(first.folder_id, None, "an insert cannot know the local folder");
        assert_eq!(first.trashed_at, None);

        move_to_folder(&conn, first.id, Some(docs.id)).unwrap();
        set_starred(&conn, first.id, true).unwrap();
        set_archived(&conn, first.id, true).unwrap();

        let mut renamed = remote("drive-1", "after.txt");
        renamed.size_bytes = Some(4096);
        renamed.md5_checksum = Some("cafe".into());
        let again = upsert_remote(&conn, account_id, &renamed).unwrap();

        assert_eq!(again.id, first.id, "the second upsert must update, not insert");
        assert_eq!(again.name, "after.txt");
        assert_eq!(again.size_bytes, Some(4096));
        assert_eq!(again.md5_checksum.as_deref(), Some("cafe"));
        assert!(again.starred, "a sync must not clear the local star");
        assert!(again.archived, "a sync must not clear the local archive");
        assert_eq!(again.folder_id, Some(docs.id), "a sync must not move the file");
        assert_eq!(
            list(&conn, &FileFilter { folder: FolderScope::Folder(docs.id), ..Default::default() })
                .unwrap()
                .len(),
            1,
            "no duplicate row"
        );
    }

    #[test]
    fn upsert_remote_is_idempotent_on_the_drive_file_id() {
        let (conn, account_id) = setup();
        let file = remote("drive-1", "same.txt");

        let first = upsert_remote(&conn, account_id, &file).unwrap();
        let second = upsert_remote(&conn, account_id, &file).unwrap();
        assert_eq!(first, second);
        assert_eq!(list(&conn, &FileFilter::default()).unwrap().len(), 1);
    }

    #[test]
    fn upsert_remote_stamps_trashed_at_from_the_remote_flag() {
        let (conn, account_id) = setup();
        let mut trashed = remote("drive-1", "gone.txt");
        trashed.trashed = true;

        let stored = upsert_remote(&conn, account_id, &trashed).unwrap();
        assert!(stored.trashed_at.is_some());
        assert_eq!(list(&conn, &FileFilter::default()).unwrap().len(), 0);
        assert_eq!(
            list(&conn, &FileFilter { trashed: true, ..Default::default() }).unwrap().len(),
            1
        );
    }

    #[test]
    fn mark_deleted_stamps_old_rows_and_skips_the_rest() {
        let (conn, account_id) = setup();
        let old = insert(&conn, &sample(account_id, "old.txt", None)).unwrap();
        let fresh = insert(&conn, &sample(account_id, "fresh.txt", None)).unwrap();
        let already = insert(&conn, &sample(account_id, "already.txt", None)).unwrap();
        age(&conn, old.id, now_unix() - 3600);
        age(&conn, already.id, now_unix() - 3600);
        trash(&conn, already.id).unwrap();
        let before = get(&conn, already.id).unwrap().trashed_at;

        let marked = mark_deleted(
            &conn,
            account_id,
            &[
                "drive-old.txt".into(),
                "drive-fresh.txt".into(),
                "drive-already.txt".into(),
                "drive-never-existed.txt".into(),
            ],
        )
        .unwrap();

        assert_eq!(marked, 1, "only the stale active row changes");
        assert!(get(&conn, old.id).unwrap().trashed_at.is_some());
        assert_eq!(
            get(&conn, fresh.id).unwrap().trashed_at,
            None,
            "a row written 10s ago is lag, not deletion"
        );
        assert_eq!(
            get(&conn, already.id).unwrap().trashed_at,
            before,
            "an already-trashed row is left alone"
        );
        assert_eq!(mark_deleted(&conn, account_id, &[]).unwrap(), 0);
    }

    #[test]
    fn mark_deleted_never_reaches_another_accounts_rows() {
        let (conn, account_id) = setup();
        let other = accounts::create(&conn, "google", "Other", "b@example.com").unwrap();
        let mine = insert(&conn, &sample(account_id, "mine.txt", None)).unwrap();
        let theirs = insert(&conn, &sample(other.id, "theirs.txt", None)).unwrap();
        age(&conn, mine.id, now_unix() - 3600);
        age(&conn, theirs.id, now_unix() - 3600);

        assert_eq!(mark_deleted(&conn, account_id, &["drive-theirs.txt".into()]).unwrap(), 0);
        assert_eq!(get(&conn, theirs.id).unwrap().trashed_at, None);
        assert_eq!(mark_deleted(&conn, other.id, &["drive-theirs.txt".into()]).unwrap(), 1);
        assert!(get(&conn, theirs.id).unwrap().trashed_at.is_some());
        assert_eq!(get(&conn, mine.id).unwrap().trashed_at, None);
    }

    fn age(conn: &Connection, id: i64, updated_at: i64) {
        conn.execute("UPDATE files SET updated_at = ?1 WHERE id = ?2", [updated_at, id]).unwrap();
    }
}
