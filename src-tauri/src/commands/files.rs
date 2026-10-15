//! Everything about one file row: listing, local flags, names, moves, the trash, and the two
//! commands that hand bytes to a blob URL or to the disk.
//!
//! The mirror is local, so most of this is SQLite. Only the trash talks to Drive, and it does
//! so *first*: `repos/files.rs` documents that a local row written before Drive has agreed is
//! un-trashed again by the next sync.

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::sync::Arc;

use drive_core::db::{self, SharedConn};
use drive_core::models::FileItem;
use drive_core::repos::files::{self, FileFilter, FolderScope};
use drive_storage::provider::StorageProvider;
use drive_storage::types::Range;
use serde::Deserialize;
use tauri::{AppHandle, State};
use tauri_plugin_dialog::DialogExt;

use crate::state::AppState;

use super::{IpcResult, invalid, ok, ok_provider, storage};

/// The one argument of `list_files`, shaped exactly like `FileQuery` in `ipc.ts`.
///
/// It exists because `repos::files::FileFilter` and `FolderScope` do not derive `Deserialize`:
/// the three folder scopes are a Rust-only distinction ("everywhere", "this folder", "unfiled")
/// and the frontend asks for them with two optional fields.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct FileQuery {
    /// `Some` lists one folder; `None` lists everything unless `loose_only` says otherwise.
    pub folder_id: Option<i64>,
    /// Files in no virtual folder. Ignored when `folder_id` names one: a caller that named a
    /// folder is asking for that folder, not for the unfiled pile.
    pub loose_only: bool,
    /// Absent means "no filter", which is why these are `Option` and not `bool`.
    pub starred: Option<bool>,
    pub archived: Option<bool>,
    /// `true` lists the trash; otherwise trashed rows stay hidden.
    pub trashed: bool,
    /// Case-insensitive substring on the name. Empty is no filter, not "match nothing".
    pub search: String,
    pub limit: Option<i64>,
}

impl FileQuery {
    /// The repo filter this query means.
    pub fn filter(&self) -> FileFilter {
        FileFilter {
            folder: match self.folder_id {
                Some(id) => FolderScope::Folder(id),
                None if self.loose_only => FolderScope::Loose,
                None => FolderScope::All,
            },
            starred: self.starred,
            archived: self.archived,
            trashed: self.trashed,
            search: (!self.search.is_empty()).then(|| self.search.clone()),
            limit: self.limit,
        }
    }
}

/// The window `preview_data` reads, inclusive on both ends, as the HTTP `Range` header spells it.
#[derive(Debug, Clone, Copy, Deserialize)]
pub struct ByteRange {
    pub start: u64,
    pub end: u64,
}

/// The subset of flags `batch_update` may flip. A field that is absent is left alone, which is
/// what `Option` buys over two booleans.
#[derive(Debug, Clone, Copy, Default, Deserialize)]
pub struct FilePatch {
    pub starred: Option<bool>,
    pub archived: Option<bool>,
}

#[tauri::command(rename_all = "snake_case")]
pub fn list_files(state: State<'_, AppState>, query: FileQuery) -> IpcResult<Vec<FileItem>> {
    list(&state.conn(), &query.filter())
}

/// The name search across every folder, kept as its own command because that is the shape
/// `ipc.ts` calls.
#[tauri::command(rename_all = "snake_case")]
pub fn search_files(
    state: State<'_, AppState>,
    query: String,
    limit: Option<i64>,
) -> IpcResult<Vec<FileItem>> {
    let filter = FileFilter {
        search: (!query.trim().is_empty()).then_some(query),
        limit,
        ..Default::default()
    };
    list(&state.conn(), &filter)
}

#[tauri::command(rename_all = "snake_case")]
pub fn get_file(state: State<'_, AppState>, file_id: i64) -> IpcResult<FileItem> {
    get(&state.conn(), file_id)
}

#[tauri::command(rename_all = "snake_case")]
pub fn set_starred(state: State<'_, AppState>, file_id: i64, starred: bool) -> IpcResult<FileItem> {
    ok(db::with_conn(&state.conn(), |conn| {
        files::set_starred(conn, file_id, starred)?;
        files::get(conn, file_id)
    }))
}

#[tauri::command(rename_all = "snake_case")]
pub fn set_archived(
    state: State<'_, AppState>,
    file_id: i64,
    archived: bool,
) -> IpcResult<FileItem> {
    ok(db::with_conn(&state.conn(), |conn| {
        files::set_archived(conn, file_id, archived)?;
        files::get(conn, file_id)
    }))
}

#[tauri::command(rename_all = "snake_case")]
pub fn rename_file(state: State<'_, AppState>, file_id: i64, name: String) -> IpcResult<()> {
    ok(db::with_conn(&state.conn(), |conn| files::rename(conn, file_id, &name)))
}

/// Applies only the flags the patch carries, to every id, in one lock scope.
#[tauri::command(rename_all = "snake_case")]
pub fn batch_update(
    state: State<'_, AppState>,
    file_ids: Vec<i64>,
    patch: FilePatch,
) -> IpcResult<()> {
    apply_patch(&state.conn(), &file_ids, patch)
}

fn apply_patch(conn: &SharedConn, file_ids: &[i64], patch: FilePatch) -> IpcResult<()> {
    ok(db::with_conn(conn, |conn| {
        for id in file_ids {
            if let Some(starred) = patch.starred {
                files::set_starred(conn, *id, starred)?;
            }
            if let Some(archived) = patch.archived {
                files::set_archived(conn, *id, archived)?;
            }
        }
        Ok(())
    }))
}

#[tauri::command(rename_all = "snake_case")]
pub fn move_files(
    state: State<'_, AppState>,
    file_ids: Vec<i64>,
    folder_id: Option<i64>,
) -> IpcResult<()> {
    ok(db::with_conn(&state.conn(), |conn| {
        // Answers `not_found` for a folder that does not exist, before anything is moved.
        if let Some(id) = folder_id {
            drive_core::repos::folders::get(conn, id)?;
        }
        for id in &file_ids {
            files::move_to_folder(conn, *id, folder_id)?;
        }
        Ok(())
    }))
}

/// Removes the mirror rows. Drive keeps the files: this is the app forgetting them, and the
/// next sync would bring them back.
#[tauri::command(rename_all = "snake_case")]
pub fn delete_files(state: State<'_, AppState>, file_ids: Vec<i64>) -> IpcResult<()> {
    ok(db::with_conn(&state.conn(), |conn| {
        for id in &file_ids {
            files::delete(conn, *id)?;
        }
        Ok(())
    }))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn trash_files(state: State<'_, AppState>, file_ids: Vec<i64>) -> IpcResult<()> {
    set_trashed(&state, file_ids, true).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn untrash_files(state: State<'_, AppState>, file_ids: Vec<i64>) -> IpcResult<()> {
    set_trashed(&state, file_ids, false).await
}

/// Drives Drive first and only writes the row once Drive agreed. A row that claimed a trash
/// Drive refused would be un-trashed by the next sync, and the user would watch the file come
/// back; a file whose Drive call fails keeps its local state and reports why.
async fn set_trashed(state: &AppState, file_ids: Vec<i64>, trashed: bool) -> IpcResult<()> {
    let rows: Vec<FileItem> = ok(db::with_conn(&state.conn(), |conn| {
        file_ids.iter().map(|id| files::get(conn, *id)).collect()
    }))?;

    // One provider per account for the whole call, not one per file.
    let mut providers: HashMap<i64, Arc<dyn StorageProvider>> = HashMap::new();
    for file in &rows {
        let provider = match providers.entry(file.account_id) {
            Entry::Occupied(seen) => Arc::clone(seen.get()),
            Entry::Vacant(slot) => {
                let provider = ok(state.provider(file.account_id).await)?;
                slot.insert(Arc::clone(&provider));
                provider
            }
        };
        provider.set_trashed(&file.drive_file_id, trashed).await.map_err(storage)?;
    }

    ok(db::with_conn(&state.conn(), |conn| {
        for file in &rows {
            if trashed {
                files::trash(conn, file.id)?;
            } else {
                files::untrash(conn, file.id)?;
            }
        }
        Ok(())
    }))
}

/// Flips the star and reports the value it landed on, so the row patches without a refetch.
#[tauri::command(rename_all = "snake_case")]
pub fn toggle_star(state: State<'_, AppState>, file_id: i64) -> IpcResult<bool> {
    toggle(&state.conn(), file_id, Flag::Star)
}

/// The archive twin of [`toggle_star`].
#[tauri::command(rename_all = "snake_case")]
pub fn toggle_archive(state: State<'_, AppState>, file_id: i64) -> IpcResult<bool> {
    toggle(&state.conn(), file_id, Flag::Archive)
}

/// Which local flag a toggle flips. An enum rather than a `bool` so the call site reads as the
/// thing it means.
#[derive(Debug, Clone, Copy)]
enum Flag {
    Star,
    Archive,
}

fn toggle(conn: &SharedConn, file_id: i64, flag: Flag) -> IpcResult<bool> {
    ok(db::with_conn(conn, |conn| {
        let current = match flag {
            Flag::Star => files::get(conn, file_id)?.starred,
            Flag::Archive => files::get(conn, file_id)?.archived,
        };
        let flipped = !current;
        match flag {
            Flag::Star => files::set_starred(conn, file_id, flipped)?,
            Flag::Archive => files::set_archived(conn, file_id, flipped)?,
        }
        Ok(flipped)
    }))
}

/// Bytes for a `blob:` URL. `range` is a seek window for a media element; without one the whole
/// file comes back, which the frontend only asks for on a small preview.
#[tauri::command(rename_all = "snake_case")]
pub async fn preview_data(
    state: State<'_, AppState>,
    file_id: i64,
    range: Option<ByteRange>,
) -> IpcResult<Vec<u8>> {
    let file = get(&state.conn(), file_id)?;
    let provider = ok(state.provider(file.account_id).await)?;
    let range = window(range, file.size_bytes)?;
    let bytes = provider.download(&file.drive_file_id, range).await.map_err(storage)?;
    Ok(bytes)
}

/// Writes the file wherever the user picked, and answers that path. Cancelling answers `None`
/// without a single Drive call, which is why the dialog is asked before anything is downloaded.
#[tauri::command(rename_all = "snake_case")]
pub async fn save_file_as(
    app: AppHandle,
    state: State<'_, AppState>,
    file_id: i64,
) -> IpcResult<Option<String>> {
    let file = get(&state.conn(), file_id)?;
    let (answer, chosen) = tokio::sync::oneshot::channel();
    app.dialog().file().set_file_name(&file.name).save_file(move |chosen| {
        let _ = answer.send(chosen);
    });
    let chosen = chosen
        .await
        .map_err(|_| invalid("the save dialog closed without answering"))?
        .map(tauri_plugin_dialog::FilePath::into_path)
        .transpose()
        .map_err(|err| invalid(format!("not a path this app can write: {err}")))?;

    // `None` is the user dismissing the dialog, not a failure.
    let Some(path) = chosen else { return Ok(None) };
    let provider = ok(state.provider(file.account_id).await)?;
    let bytes = provider.download(&file.drive_file_id, None).await.map_err(storage)?;
    // shortcut: the whole body is buffered before the write because `StorageProvider::download`
    // returns `Vec<u8>`; stream it when the trait does (same limit drive-storage documents).
    std::fs::write(&path, bytes)
        .map_err(|err| invalid(format!("could not write {}: {err}", path.display())))?;
    Ok(Some(path.to_string_lossy().into_owned()))
}

/// Clamps an inclusive byte range to what the file actually has. A seek past the end asks for
/// the tail rather than nothing, because a media element waiting on an empty answer stalls;
/// a start past the end is a caller bug and says so. A file whose size Drive never reported is
/// passed through untouched.
fn window(range: Option<ByteRange>, size_bytes: Option<i64>) -> IpcResult<Option<Range>> {
    let Some(range) = range else { return Ok(None) };
    let Some(size) = size_bytes.and_then(|size| u64::try_from(size).ok()) else {
        return Ok(Some(ok_provider(Range::new(range.start, range.end))?));
    };
    if range.start >= size {
        return Err(invalid(format!(
            "range starts at {} but the file is {size} bytes",
            range.start
        )));
    }
    Ok(Some(ok_provider(Range::new(range.start, range.end.min(size - 1)))?))
}

fn list(conn: &SharedConn, filter: &FileFilter) -> IpcResult<Vec<FileItem>> {
    ok(db::with_conn(conn, |c| files::list(c, filter)))
}

fn get(conn: &SharedConn, file_id: i64) -> IpcResult<FileItem> {
    ok(db::with_conn(conn, |c| files::get(c, file_id)))
}

#[cfg(test)]
mod tests {
    use drive_core::repos::accounts;

    use super::*;

    fn conn() -> SharedConn {
        db::shared(db::open_in_memory().expect("in-memory database"))
    }

    fn account(conn: &SharedConn) -> i64 {
        db::with_conn(conn, |c| Ok(accounts::create(c, "google", "User", "a@example.com")?.id))
            .expect("seed an account")
    }

    fn seed(conn: &SharedConn, account_id: i64, name: &str, folder_id: Option<i64>) -> i64 {
        db::with_conn(conn, |c| {
            Ok(files::insert(
                c,
                &FileItem {
                    id: 0,
                    account_id,
                    drive_file_id: format!("drive-{name}"),
                    folder_id,
                    name: name.into(),
                    mime_type: None,
                    size_bytes: Some(10),
                    md5_checksum: None,
                    starred: false,
                    archived: false,
                    trashed_at: None,
                    created_at: 0,
                    updated_at: 0,
                },
            )?
            .id)
        })
        .expect("seed a file")
    }

    fn flag(conn: &SharedConn, file_id: i64) -> (bool, bool) {
        let row = db::with_conn(conn, |c| files::get(c, file_id)).expect("row");
        (row.starred, row.archived)
    }

    fn filter(query: &FileQuery) -> FileFilter {
        query.filter()
    }

    #[test]
    fn an_empty_query_lists_every_active_file_from_every_folder() {
        let filter = filter(&FileQuery::default());
        assert_eq!(filter.folder, FolderScope::All);
        assert_eq!(filter.starred, None, "an absent flag is no filter, not `false`");
        assert_eq!(filter.archived, None);
        assert!(!filter.trashed);
        assert_eq!(filter.search, None);
        assert_eq!(filter.limit, None);
    }

    #[test]
    fn a_named_folder_wins_over_loose_only() {
        let query = FileQuery { folder_id: Some(7), loose_only: true, ..Default::default() };
        assert_eq!(filter(&query).folder, FolderScope::Folder(7));

        let loose = FileQuery { loose_only: true, ..Default::default() };
        assert_eq!(filter(&loose).folder, FolderScope::Loose);
    }

    #[test]
    fn a_flag_is_only_a_filter_when_the_frontend_sent_one() {
        let starred = FileQuery { starred: Some(true), ..Default::default() };
        assert_eq!(filter(&starred).starred, Some(true));
        assert_eq!(filter(&starred).archived, None);

        let unstarred = FileQuery { starred: Some(false), ..Default::default() };
        assert_eq!(filter(&unstarred).starred, Some(false), "false is a filter, not an absence");

        let archive_page = FileQuery { archived: Some(true), ..Default::default() };
        assert_eq!(filter(&archive_page).archived, Some(true));

        let trash = FileQuery { trashed: true, ..Default::default() };
        assert!(filter(&trash).trashed);
    }

    #[test]
    fn an_empty_search_is_no_filter_not_a_term_that_matches_nothing() {
        let cleared = FileQuery { search: String::new(), ..Default::default() };
        assert_eq!(filter(&cleared).search, None);

        let typed = FileQuery { search: "  report  ".into(), ..Default::default() };
        assert_eq!(filter(&typed).search.as_deref(), Some("  report  "));
    }

    #[test]
    fn the_limit_is_passed_through() {
        assert_eq!(filter(&FileQuery { limit: Some(25), ..Default::default() }).limit, Some(25));
    }

    #[test]
    fn a_query_narrowing_two_flags_narrows_further_than_one() {
        let conn = conn();
        let account_id = account(&conn);
        let folder_id =
            db::with_conn(&conn, |c| Ok(drive_core::repos::folders::create(c, "Docs", None)?.id))
                .expect("folder");
        let both = seed(&conn, account_id, "both.txt", Some(folder_id));
        let starred = seed(&conn, account_id, "starred.txt", Some(folder_id));
        let plain = seed(&conn, account_id, "plain.txt", Some(folder_id));
        db::with_conn(&conn, |c| {
            files::set_starred(c, both, true)?;
            files::set_archived(c, both, true)?;
            files::set_starred(c, starred, true)?;
            Ok(())
        })
        .expect("flags");

        let query =
            FileQuery { folder_id: Some(folder_id), starred: Some(true), ..Default::default() };
        let listed = list(&conn, &filter(&query)).expect("listed");
        assert_eq!(listed.len(), 2);

        let both_flags = FileQuery {
            folder_id: Some(folder_id),
            starred: Some(true),
            archived: Some(true),
            ..Default::default()
        };
        let narrowed = list(&conn, &filter(&both_flags)).expect("listed");
        assert_eq!(narrowed.len(), 1);
        assert_eq!(narrowed[0].id, both);
        assert!(narrowed.iter().all(|file| file.id != plain));
    }

    #[test]
    fn a_toggle_reports_the_value_it_landed_on_and_alternates() {
        let conn = conn();
        let file_id = seed(&conn, account(&conn), "star.txt", None);

        assert!(toggle(&conn, file_id, Flag::Star).expect("toggle"));
        assert!(db::with_conn(&conn, |c| files::get(c, file_id)).expect("row").starred);
        assert!(!toggle(&conn, file_id, Flag::Star).expect("toggle"));
        assert!(!db::with_conn(&conn, |c| files::get(c, file_id)).expect("row").starred);
    }

    #[test]
    fn the_two_toggles_do_not_touch_each_others_flag() {
        let conn = conn();
        let file_id = seed(&conn, account(&conn), "flags.txt", None);

        toggle(&conn, file_id, Flag::Star).expect("star");
        let row = db::with_conn(&conn, |c| files::get(c, file_id)).expect("row");
        assert!(row.starred);
        assert!(!row.archived);

        toggle(&conn, file_id, Flag::Archive).expect("archive");
        let row = db::with_conn(&conn, |c| files::get(c, file_id)).expect("row");
        assert!(row.starred, "starring again would clear the star");
        assert!(row.archived);
    }

    #[test]
    fn toggling_a_missing_file_is_not_found() {
        let conn = conn();
        assert_eq!(toggle(&conn, 404, Flag::Star).expect_err("gone").code, "not_found");
    }

    #[test]
    fn a_patch_applies_only_the_fields_it_carries() {
        let conn = conn();
        let file_id = seed(&conn, account(&conn), "patched.txt", None);
        db::with_conn(&conn, |c| {
            files::set_archived(c, file_id, true)?;
            Ok(())
        })
        .expect("archive");

        // Star only: the archive stays.
        apply_patch(&conn, &[file_id], FilePatch { starred: Some(true), archived: None })
            .expect("patch");
        assert_eq!(flag(&conn, file_id), (true, true), "an absent field leaves the flag alone");

        // An empty patch changes nothing at all.
        apply_patch(&conn, &[file_id], FilePatch::default()).expect("patch");
        assert_eq!(flag(&conn, file_id), (true, true));

        // Unstarring through a patch is a filter value, not an absence.
        apply_patch(&conn, &[file_id], FilePatch { starred: Some(false), archived: None })
            .expect("patch");
        assert_eq!(flag(&conn, file_id), (false, true));
    }

    #[test]
    fn a_patch_reaches_every_id_it_was_given() {
        let conn = conn();
        let account_id = account(&conn);
        let first = seed(&conn, account_id, "a.txt", None);
        let second = seed(&conn, account_id, "b.txt", None);

        apply_patch(&conn, &[first, second], FilePatch { archived: Some(true), starred: None })
            .expect("patch");

        assert_eq!(flag(&conn, first), (false, true));
        assert_eq!(flag(&conn, second), (false, true));
    }

    #[test]
    fn a_patch_naming_a_missing_file_is_not_found() {
        let conn = conn();
        let err = apply_patch(&conn, &[404], FilePatch { starred: Some(true), archived: None })
            .expect_err("gone");
        assert_eq!(err.code, "not_found");
    }

    #[test]
    fn a_range_past_the_end_is_clamped_to_the_last_byte() {
        let range = Some(ByteRange { start: 5, end: 9_999 });
        let clamped = window(range, Some(10)).expect("clamped").expect("a window");
        assert_eq!(clamped.start(), 5);
        assert_eq!(clamped.end(), 9, "inclusive, and inside the file");
    }

    #[test]
    fn a_range_inside_the_file_is_left_alone() {
        let clamped =
            window(Some(ByteRange { start: 0, end: 3 }), Some(10)).expect("ok").expect("w");
        assert_eq!((clamped.start(), clamped.end()), (0, 3));

        let whole = window(Some(ByteRange { start: 0, end: 9 }), Some(10)).expect("ok").expect("w");
        assert_eq!(whole.end(), 9);
    }

    #[test]
    fn a_start_past_the_end_is_rejected_rather_than_returning_nothing() {
        let err = window(Some(ByteRange { start: 10, end: 20 }), Some(10)).expect_err("past end");
        assert_eq!(err.code, "invalid_input");
    }

    #[test]
    fn an_empty_file_has_no_byte_window_to_clamp_into() {
        let err = window(Some(ByteRange { start: 0, end: 0 }), Some(0)).expect_err("empty");
        assert_eq!(err.code, "invalid_input");
    }

    #[test]
    fn a_file_of_unknown_size_passes_the_range_through() {
        let clamped = window(Some(ByteRange { start: 10, end: 20 }), None).expect("ok").expect("w");
        assert_eq!((clamped.start(), clamped.end()), (10, 20));
    }

    #[test]
    fn an_absent_range_means_the_whole_file() {
        assert!(window(None, Some(10)).expect("ok").is_none());
    }

    #[test]
    fn an_inverted_range_is_the_callers_bug() {
        let err = window(Some(ByteRange { start: 9, end: 4 }), Some(10)).expect_err("inverted");
        assert_eq!(err.code, "invalid_argument");
    }

    #[test]
    fn an_absent_flag_field_is_none_rather_than_false() {
        // The frontend sends `{ starred: true }` with no `archived` key, so an absent field has
        // to mean "leave it alone" -- which is exactly what `Option` gives over a `bool`.
        assert_eq!(FilePatch::default().starred, None);
        assert_eq!(FilePatch::default().archived, None);
        assert!(!FileQuery::default().loose_only);
        assert!(!FileQuery::default().trashed);
    }
}
