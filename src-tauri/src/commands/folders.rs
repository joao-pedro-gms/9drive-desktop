//! The virtual folder tree, which is local SQLite only and never touches Google Drive
//! (`ipc.ts` says so, and the legacy app agrees: deleting a virtual folder must not delete
//! anything in Drive).

use std::collections::HashMap;

use drive_core::db::{self, SharedConn};
use drive_core::models::{FileItem, Folder};
use drive_core::repos::files::{self, FileFilter};
use drive_core::repos::folders;
use tauri::State;

use crate::state::AppState;

use super::{IpcResult, ok};

/// How many entries `recent_folders` returns: a sidebar list, not the whole tree.
const RECENT_LIMIT: usize = 10;

#[tauri::command(rename_all = "snake_case")]
pub fn list_folders(state: State<'_, AppState>, parent_id: Option<i64>) -> IpcResult<Vec<Folder>> {
    list_by_parent(&state.conn(), parent_id)
}

#[tauri::command(rename_all = "snake_case")]
pub fn create_folder(
    state: State<'_, AppState>,
    parent_id: Option<i64>,
    name: String,
) -> IpcResult<Folder> {
    create(&state.conn(), parent_id, &name)
}

#[tauri::command(rename_all = "snake_case")]
pub fn rename_folder(state: State<'_, AppState>, folder_id: i64, name: String) -> IpcResult<()> {
    rename(&state.conn(), folder_id, &name)
}

#[tauri::command(rename_all = "snake_case")]
pub fn move_folder(
    state: State<'_, AppState>,
    folder_id: i64,
    parent_id: Option<i64>,
) -> IpcResult<()> {
    move_to(&state.conn(), folder_id, parent_id)
}

/// Removes the virtual folder, its children and the files' folder pointer. Nothing is sent to
/// Drive: the tree is local, and a Drive-side delete here would take real user data with it.
#[tauri::command(rename_all = "snake_case")]
pub fn delete_folder(state: State<'_, AppState>, folder_id: i64) -> IpcResult<()> {
    delete(&state.conn(), folder_id)
}

/// Folders with something in them, most recently touched first.
#[tauri::command(rename_all = "snake_case")]
pub fn recent_folders(state: State<'_, AppState>) -> IpcResult<Vec<Folder>> {
    let (all, active) = ok(db::with_conn(&state.conn(), |conn| {
        Ok((folders::list(conn)?, files::list(conn, &FileFilter::default())?))
    }))?;
    Ok(recent(&all, &active))
}

fn list_by_parent(conn: &SharedConn, parent_id: Option<i64>) -> IpcResult<Vec<Folder>> {
    ok(db::with_conn(conn, |c| folders::list_by_parent(c, parent_id)))
}

fn create(conn: &SharedConn, parent_id: Option<i64>, name: &str) -> IpcResult<Folder> {
    // `folders::create` rejects a blank name and a parent that does not exist.
    ok(db::with_conn(conn, |c| folders::create(c, name, parent_id)))
}

fn rename(conn: &SharedConn, folder_id: i64, name: &str) -> IpcResult<()> {
    ok(db::with_conn(conn, |c| folders::rename(c, folder_id, name)))
}

/// The cycle guard is `folders::move_to`'s: it answers `conflict` for a move into the folder's
/// own subtree, and leaves the row untouched when it refuses.
fn move_to(conn: &SharedConn, folder_id: i64, parent_id: Option<i64>) -> IpcResult<()> {
    ok(db::with_conn(conn, |c| folders::move_to(c, folder_id, parent_id)))
}

fn delete(conn: &SharedConn, folder_id: i64) -> IpcResult<()> {
    ok(db::with_conn(conn, |c| folders::delete(c, folder_id)))
}

/// Derived rather than tracked: nothing records "the user browsed this folder", and adding a
/// visited-at column is schema work this command does not own. The newest file write into a
/// folder is the best proxy available, and an empty folder has nothing to show anyway.
fn recent(all: &[Folder], active: &[FileItem]) -> Vec<Folder> {
    let mut newest: HashMap<i64, i64> = HashMap::new();
    for file in active {
        let Some(folder_id) = file.folder_id else { continue };
        newest
            .entry(folder_id)
            .and_modify(|seen| *seen = (*seen).max(file.updated_at))
            .or_insert(file.updated_at);
    }

    let mut recent: Vec<Folder> =
        all.iter().filter(|folder| newest.contains_key(&folder.id)).cloned().collect();
    // The id breaks a tie, so two files stamped in the same second never reorder between runs.
    recent.sort_by(|a, b| newest[&b.id].cmp(&newest[&a.id]).then(a.id.cmp(&b.id)));
    recent.truncate(RECENT_LIMIT);
    recent
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(id: i64, name: &str) -> Folder {
        Folder { id, parent_id: None, name: name.into(), created_at: 0, updated_at: 0 }
    }

    fn file(folder_id: Option<i64>, updated_at: i64) -> FileItem {
        FileItem {
            id: updated_at,
            account_id: 1,
            drive_file_id: format!("drive-{updated_at}"),
            folder_id,
            name: format!("f{updated_at}.txt"),
            mime_type: None,
            size_bytes: None,
            md5_checksum: None,
            starred: false,
            archived: false,
            trashed_at: None,
            created_at: 0,
            updated_at,
        }
    }

    fn names(folders: &[Folder]) -> Vec<&str> {
        folders.iter().map(|folder| folder.name.as_str()).collect()
    }

    #[test]
    fn the_newest_touched_folder_comes_first_and_empties_drop_out() {
        let all = [folder(1, "Old"), folder(2, "Recent"), folder(3, "Empty")];
        let files = [file(Some(1), 100), file(Some(2), 300)];

        assert_eq!(names(&recent(&all, &files)), ["Recent", "Old"]);
    }

    #[test]
    fn a_file_in_no_folder_is_not_a_folder() {
        let all = [folder(1, "Docs")];
        assert!(recent(&all, &[file(None, 300)]).is_empty());
    }

    #[test]
    fn a_folders_recency_is_its_newest_file_not_its_oldest() {
        let all = [folder(1, "A"), folder(2, "B")];
        let files = [file(Some(1), 100), file(Some(1), 900), file(Some(2), 500)];

        assert_eq!(names(&recent(&all, &files)), ["A", "B"]);
    }

    #[test]
    fn files_stamped_the_same_second_do_not_reorder_between_runs() {
        let all = [folder(7, "B"), folder(3, "A")];
        let files = [file(Some(7), 100), file(Some(3), 100)];

        // Lowest id first on a tie, every time.
        assert_eq!(names(&recent(&all, &files)), ["A", "B"]);
    }

    #[test]
    fn the_sidebar_list_is_capped() {
        let all: Vec<Folder> = (0..(RECENT_LIMIT as i64 + 5)).map(|id| folder(id, "f")).collect();
        let files: Vec<FileItem> =
            (0..(RECENT_LIMIT as i64 + 5)).map(|id| file(Some(id), 100 + id)).collect();

        assert_eq!(recent(&all, &files).len(), RECENT_LIMIT);
    }

    #[test]
    fn an_empty_tree_has_no_recent_folders() {
        assert!(recent(&[folder(1, "Docs")], &[]).is_empty());
        assert!(recent(&[], &[]).is_empty());
    }

    #[test]
    fn a_move_into_own_subtree_is_a_conflict_and_changes_nothing() {
        let conn = db::shared(db::open_in_memory().expect("in-memory database"));
        let parent = create(&conn, None, "Parent").expect("create");
        let child = create(&conn, Some(parent.id), "Child").expect("create");

        let err = move_to(&conn, parent.id, Some(child.id)).expect_err("a cycle must be refused");
        assert_eq!(err.code, "conflict");

        let listed = list_by_parent(&conn, None).expect("roots");
        assert_eq!(listed.len(), 1, "the refused move must not have persisted");
        assert_eq!(listed[0].name, "Parent");
    }

    #[test]
    fn a_blank_name_and_a_missing_parent_are_reported_not_swallowed() {
        let conn = db::shared(db::open_in_memory().expect("in-memory database"));
        assert_eq!(create(&conn, None, "  ").expect_err("blank").code, "invalid_input");
        assert_eq!(create(&conn, Some(404), "Docs").expect_err("no parent").code, "not_found");
        assert_eq!(rename(&conn, 404, "Docs").expect_err("no folder").code, "not_found");
        assert_eq!(delete(&conn, 404).expect_err("no folder").code, "not_found");
    }
}
