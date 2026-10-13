use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::db::now_unix;
use crate::error::{AppError, Result};
use crate::models::Folder;

const COLUMNS: &str = "id, parent_id, name, created_at, updated_at";

fn map(row: &Row<'_>) -> rusqlite::Result<Folder> {
    Ok(Folder {
        id: row.get("id")?,
        parent_id: row.get("parent_id")?,
        name: row.get("name")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}

pub fn list(conn: &Connection) -> Result<Vec<Folder>> {
    let sql = format!("SELECT {COLUMNS} FROM folders ORDER BY name, id");
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], map)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Direct children of `parent_id`; `None` returns the roots.
pub fn list_by_parent(conn: &Connection, parent_id: Option<i64>) -> Result<Vec<Folder>> {
    let sql = format!("SELECT {COLUMNS} FROM folders WHERE parent_id IS ?1 ORDER BY name, id");
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([parent_id], map)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

pub fn get(conn: &Connection, id: i64) -> Result<Folder> {
    let sql = format!("SELECT {COLUMNS} FROM folders WHERE id = ?1");
    match conn.query_row(&sql, [id], map) {
        Ok(folder) => Ok(folder),
        Err(rusqlite::Error::QueryReturnedNoRows) => Err(AppError::not_found("folder", id)),
        Err(other) => Err(AppError::Database(other)),
    }
}

pub fn create(conn: &Connection, name: &str, parent_id: Option<i64>) -> Result<Folder> {
    validate_name(name)?;
    if let Some(parent) = parent_id {
        get(conn, parent)?;
    }
    let now = now_unix();
    conn.execute(
        "INSERT INTO folders (parent_id, name, created_at, updated_at) VALUES (?1, ?2, ?3, ?3)",
        params![parent_id, name, now],
    )?;
    get(conn, conn.last_insert_rowid())
}

pub fn rename(conn: &Connection, id: i64, name: &str) -> Result<()> {
    validate_name(name)?;
    touch(conn, id, "name = ?2", params![id, name, now_unix()])
}

/// Rejects a move that would put a folder inside its own subtree.
pub fn move_to(conn: &Connection, id: i64, new_parent: Option<i64>) -> Result<()> {
    get(conn, id)?;
    ensure_not_ancestor(conn, id, new_parent)?;
    touch(conn, id, "parent_id = ?2", params![id, new_parent, now_unix()])
}

/// Child folders go with it (ON DELETE CASCADE) and its files lose their folder.
pub fn delete(conn: &Connection, id: i64) -> Result<()> {
    let changed = conn.execute("DELETE FROM folders WHERE id = ?1", [id])?;
    if changed == 0 {
        return Err(AppError::not_found("folder", id));
    }
    Ok(())
}

/// `args` carries `?1` = id, `?2` = the assigned value and `?3` = updated_at.
fn touch(conn: &Connection, id: i64, assignment: &str, args: impl rusqlite::Params) -> Result<()> {
    let sql = format!("UPDATE folders SET {assignment}, updated_at = ?3 WHERE id = ?1");
    let changed = conn.execute(&sql, args)?;
    if changed == 0 {
        return Err(AppError::not_found("folder", id));
    }
    Ok(())
}

fn validate_name(name: &str) -> Result<()> {
    if name.trim().is_empty() {
        return Err(AppError::InvalidInput("folder name must not be blank".into()));
    }
    Ok(())
}

/// Walks up from the proposed parent instead of down from `id`: cheaper, and the only
/// cycle that matters is the one ending at `id`. The hop limit keeps a pre-existing
/// malformed cycle from spinning forever.
fn ensure_not_ancestor(conn: &Connection, id: i64, parent: Option<i64>) -> Result<()> {
    let limit: i64 = conn.query_row("SELECT COUNT(*) FROM folders", [], |row| row.get(0))?;
    let mut cursor = parent;
    let mut hops = 0;
    while let Some(current) = cursor {
        if current == id {
            return Err(AppError::Conflict(format!(
                "folder {id} cannot be moved into itself or one of its descendants"
            )));
        }
        hops += 1;
        if hops > limit {
            return Err(AppError::Conflict("folder tree already contains a cycle".into()));
        }
        cursor = match parent_of(conn, current)? {
            Some(parent) => parent,
            None => {
                return Err(AppError::InvalidInput(format!(
                    "parent folder {current} does not exist"
                )));
            }
        };
    }
    Ok(())
}

/// `Ok(None)` for a row with no parent, `Err` when the row does not exist.
fn parent_of(conn: &Connection, id: i64) -> Result<Option<Option<i64>>> {
    Ok(conn
        .query_row("SELECT parent_id FROM folders WHERE id = ?1", [id], |row| row.get(0))
        .optional()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_conn;
    use crate::repos::accounts;
    use crate::repos::files;

    #[test]
    fn create_get_and_list() {
        let conn = test_conn();
        let root = create(&conn, "Documents", None).unwrap();
        assert!(root.id > 0);
        assert_eq!(root.parent_id, None);
        assert!(root.created_at > 0 && root.updated_at >= root.created_at);

        let child = create(&conn, "Reports", Some(root.id)).unwrap();
        assert_eq!(child.parent_id, Some(root.id));
        assert_eq!(get(&conn, child.id).unwrap(), child);
        assert_eq!(list(&conn).unwrap().len(), 2);
    }

    #[test]
    fn list_by_parent_returns_only_that_level() {
        let conn = test_conn();
        let root = create(&conn, "Documents", None).unwrap();
        let a = create(&conn, "Alpha", Some(root.id)).unwrap();
        create(&conn, "Beta", Some(root.id)).unwrap();
        create(&conn, "Root-only", None).unwrap();
        let grandchild = create(&conn, "Deep", Some(a.id)).unwrap();

        let children: Vec<i64> =
            list_by_parent(&conn, Some(root.id)).unwrap().into_iter().map(|f| f.id).collect();
        assert_eq!(children.len(), 2);
        assert!(children.contains(&a.id));
        assert!(!children.contains(&grandchild.id));

        let roots = list_by_parent(&conn, None).unwrap();
        assert_eq!(roots.len(), 2);
        assert!(roots.iter().all(|f| f.parent_id.is_none()));
    }

    #[test]
    fn rename_rejects_blank_and_missing_folder() {
        let conn = test_conn();
        let folder = create(&conn, "Documents", None).unwrap();

        rename(&conn, folder.id, "Papers").unwrap();
        assert_eq!(get(&conn, folder.id).unwrap().name, "Papers");
        assert!(matches!(rename(&conn, folder.id, "   "), Err(AppError::InvalidInput(_))));
        assert!(matches!(create(&conn, "", None), Err(AppError::InvalidInput(_))));
        assert!(matches!(rename(&conn, 999, "x"), Err(AppError::NotFound { .. })));
    }

    #[test]
    fn move_to_rewires_the_tree() {
        let conn = test_conn();
        let src = create(&conn, "Src", None).unwrap();
        let dst = create(&conn, "Dst", None).unwrap();
        let folder = create(&conn, "Movable", Some(src.id)).unwrap();

        move_to(&conn, folder.id, Some(dst.id)).unwrap();
        assert_eq!(get(&conn, folder.id).unwrap().parent_id, Some(dst.id));
        move_to(&conn, folder.id, None).unwrap();
        assert_eq!(get(&conn, folder.id).unwrap().parent_id, None);
        assert_eq!(list_by_parent(&conn, Some(src.id)).unwrap().len(), 0);
    }

    #[test]
    fn move_to_rejects_cycles() {
        let conn = test_conn();
        let a = create(&conn, "A", None).unwrap();
        let b = create(&conn, "B", Some(a.id)).unwrap();
        let c = create(&conn, "C", Some(b.id)).unwrap();

        assert!(matches!(move_to(&conn, a.id, Some(a.id)), Err(AppError::Conflict(_))));
        assert!(matches!(move_to(&conn, a.id, Some(c.id)), Err(AppError::Conflict(_))));
        assert!(matches!(move_to(&conn, b.id, Some(c.id)), Err(AppError::Conflict(_))));
        assert_eq!(get(&conn, a.id).unwrap().parent_id, None, "a rejected move must not persist");

        move_to(&conn, c.id, Some(a.id)).unwrap();
        assert_eq!(get(&conn, c.id).unwrap().parent_id, Some(a.id));
    }

    #[test]
    fn move_to_rejects_unknown_parent_and_folder() {
        let conn = test_conn();
        let folder = create(&conn, "A", None).unwrap();
        assert!(matches!(move_to(&conn, folder.id, Some(404)), Err(AppError::InvalidInput(_))));
        assert!(matches!(move_to(&conn, 404, Some(folder.id)), Err(AppError::NotFound { .. })));
    }

    #[test]
    fn delete_cascades_to_children_and_nulls_files() {
        let conn = test_conn();
        let account = accounts::create(&conn, "google", "User", "a@example.com").unwrap();
        let parent = create(&conn, "Parent", None).unwrap();
        let child = create(&conn, "Child", Some(parent.id)).unwrap();
        let file =
            files::insert(&conn, &files::sample(account.id, "doc.txt", Some(parent.id))).unwrap();
        let in_child =
            files::insert(&conn, &files::sample(account.id, "nested.txt", Some(child.id))).unwrap();

        delete(&conn, parent.id).unwrap();

        assert!(matches!(get(&conn, child.id), Err(AppError::NotFound { .. })));
        assert!(matches!(delete(&conn, parent.id), Err(AppError::NotFound { .. })));

        let orphan = files::get(&conn, file.id).unwrap();
        assert_eq!(orphan.folder_id, None);
        assert_eq!(files::get(&conn, in_child.id).unwrap().folder_id, None);
        assert_eq!(list_by_parent(&conn, None).unwrap().len(), 0);
    }
}
