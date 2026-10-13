//! Folder endpoints: the app folder lookup, folder creation, and folder listing.
//!
//! One `files.list` search, one `files.create`, one paged `files.list`. The `impl
//! StorageProvider` forwards to these.

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::types::FolderListing;

use super::GoogleDrive;
use super::map::{DriveFile, FILE_FIELDS, remote_file};

/// Drive's own folder type. A *shortcut* to a folder is
/// `application/vnd.google-apps.shortcut` and must never satisfy this filter.
const FOLDER_MIME: &str = "application/vnd.google-apps.folder";

/// Drive spells the root as this alias, not as a real file id.
const ROOT: &str = "root";

/// Drive's own default page size; the sync layer pages with [`GoogleDrive::list_page`].
const PAGE_SIZE: u32 = 100;

/// One page of `files.list`.
#[derive(Deserialize)]
struct FileList {
    #[serde(default)]
    files: Vec<DriveFile>,
    #[serde(default, rename = "nextPageToken")]
    next_page_token: Option<String>,
}

/// `files.create` with the field mask narrowed to the one field this module returns.
#[derive(Deserialize)]
struct CreatedFile {
    id: String,
}

/// `parents` is skipped at the root: Drive reads the absence of a parent, not the literal
/// `root` string, as "the root".
#[derive(Serialize)]
struct NewFolder<'a> {
    name: &'a str,
    #[serde(rename = "mimeType")]
    mime_type: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    parents: Option<Vec<&'a str>>,
}

impl GoogleDrive {
    /// shortcut: search-then-create, so two concurrent callers can both create the folder and
    /// Drive allows duplicate names; single-threaded callers (every path in F5) are safe.
    pub async fn ensure_app_folder(&self, name: &str) -> Result<String> {
        let q = format!(
            "name = '{}' and mimeType = '{FOLDER_MIME}' and trashed = false and 'root' in parents",
            q_literal(name)
        );
        let found: FileList =
            self.client.get_json("files", &[("q", &q), ("fields", "files(id)")]).await?;
        if let Some(file) = found.files.into_iter().next() {
            return Ok(file.id);
        }
        self.create_folder(ROOT, name).await
    }

    pub async fn create_folder(&self, parent_id: &str, name: &str) -> Result<String> {
        let body = NewFolder {
            name,
            mime_type: FOLDER_MIME,
            parents: (parent_id != ROOT).then(|| vec![parent_id]),
        };
        let created: CreatedFile =
            self.client.post_json("files", &[("fields", "id")], &body).await?;
        Ok(created.id)
    }

    /// The first page, for callers with nothing to continue from.
    pub async fn list_folder(&self, folder_id: &str) -> Result<FolderListing> {
        self.list_page(folder_id, PAGE_SIZE, None).await
    }

    /// One page only; the caller follows [`FolderListing::next_page_token`].
    pub async fn list_page(
        &self,
        folder_id: &str,
        page_size: u32,
        page_token: Option<&str>,
    ) -> Result<FolderListing> {
        let q = format!("'{folder_id}' in parents and trashed = false");
        let fields = format!("nextPageToken,files({FILE_FIELDS})");
        let page_size = page_size.to_string();
        let mut query =
            vec![("q", q.as_str()), ("fields", fields.as_str()), ("pageSize", page_size.as_str())];
        if let Some(token) = page_token {
            query.push(("pageToken", token));
        }

        let page: FileList = self.client.get_json("files", &query).await?;
        Ok(FolderListing {
            entries: page.files.iter().map(remote_file).collect(),
            next_page_token: page.next_page_token,
        })
    }
}

/// Escapes a folder name for Drive's query language: inside a quoted literal a `'` is written
/// `\'`, and a backslash itself is doubled. Hand-escaping the delimiters too would produce a
/// query Drive accepts and answers with zero rows.
fn q_literal(raw: &str) -> String {
    raw.replace('\\', r"\\").replace('\'', r"\'")
}

#[cfg(test)]
mod tests {
    use super::super::stub::{Reply, Stub};
    use super::*;

    const TOKEN: &str = "ya29.test-token";

    fn drive(stub: &Stub) -> GoogleDrive {
        GoogleDrive::with_endpoints(TOKEN, &stub.base, &stub.base)
    }

    /// The exact `q` `ensure_app_folder` must build for `"9drive"`.
    const APP_Q: &str = concat!(
        "name = '9drive' and mimeType = 'application/vnd.google-apps.folder' ",
        "and trashed = false and 'root' in parents"
    );

    #[tokio::test]
    async fn an_existing_app_folder_is_found_without_creating_anything() {
        let stub = Stub::start(vec![Reply::json(200, r#"{"files":[{"id":"app-1"}]}"#)]).await;
        let drive = drive(&stub);

        assert_eq!(drive.ensure_app_folder("9drive").await.expect("ensure"), "app-1");

        let requests = stub.requests();
        assert_eq!(stub.request_count(), 1);
        assert_eq!(requests[0].method, "GET");
        assert_eq!(requests[0].path, "/files");
        assert_eq!(requests[0].query("q").as_deref(), Some(APP_Q));
    }

    #[tokio::test]
    async fn an_absent_app_folder_is_created_at_the_root() {
        let stub = Stub::start(vec![
            Reply::json(200, r#"{"files":[]}"#),
            Reply::json(200, r#"{"id":"new"}"#),
        ])
        .await;
        let drive = drive(&stub);

        assert_eq!(drive.ensure_app_folder("9drive").await.expect("ensure"), "new");

        let requests = stub.requests();
        assert_eq!(stub.request_count(), 2);
        assert_eq!(requests[0].method, "GET");
        assert_eq!(requests[0].query("q").as_deref(), Some(APP_Q));
        assert_eq!(requests[1].method, "POST");
        assert_eq!(requests[1].path, "/files");
        assert!(requests[1].body.contains(r#""name":"9drive""#), "{}", requests[1].body);
        // A create at the root is the absence of a parent, not `parents: ["root"]`.
        assert!(!requests[1].body.contains("parents"), "{}", requests[1].body);
    }

    #[tokio::test]
    async fn the_app_folder_search_excludes_trashed_and_shortcut_entries() {
        let stub = Stub::start(vec![
            Reply::json(200, r#"{"files":[]}"#),
            Reply::json(200, r#"{"id":"new"}"#),
        ])
        .await;
        let drive = drive(&stub);

        drive.ensure_app_folder("9drive").await.expect("ensure");

        let q = stub.requests()[0].query("q").expect("q");
        assert!(q.contains("trashed = false"), "{q}");
        assert!(q.contains(&format!("mimeType = '{FOLDER_MIME}'")), "{q}");
        // A shortcut to a folder carries this mime type instead, and must never match.
        assert!(!q.contains("application/vnd.google-apps.shortcut"), "{q}");
    }

    #[tokio::test]
    async fn a_name_with_an_apostrophe_and_a_space_reaches_drive_intact() {
        let stub = Stub::start(vec![Reply::json(200, r#"{"files":[{"id":"app-2"}]}"#)]).await;
        let drive = drive(&stub);

        drive.ensure_app_folder("Bob's reports").await.expect("ensure");

        let request = &stub.requests()[0];
        let expected = concat!(
            r"name = 'Bob\'s reports' and mimeType = 'application/vnd.google-apps.folder' ",
            "and trashed = false and 'root' in parents"
        );
        assert_eq!(request.query("q").as_deref(), Some(expected));
        // Encoded strictly, so the escaped quote survives as a backslash Drive can read.
        assert!(request.target.contains("q=name%20%3D%20%27Bob%5C%27s"), "{}", request.target);
        assert!(request.target.contains("reports%27%20and"), "{}", request.target);
    }

    #[test]
    fn a_query_literal_escapes_the_backslash_before_the_quote() {
        assert_eq!(q_literal("plain"), "plain");
        assert_eq!(q_literal("Bob's"), r"Bob\'s");
        assert_eq!(q_literal(r"back\slash'"), r"back\\slash\'");
    }

    #[tokio::test]
    async fn create_folder_posts_the_folder_mime_type_under_its_parent() {
        let stub = Stub::start(vec![Reply::json(200, r#"{"id":"child-1"}"#)]).await;
        let drive = drive(&stub);

        assert_eq!(drive.create_folder("parent-1", "Docs").await.expect("create"), "child-1");

        let request = &stub.requests()[0];
        assert_eq!(request.method, "POST");
        assert_eq!(request.path, "/files");
        assert_eq!(request.query("fields").as_deref(), Some("id"));
        assert!(request.body.contains(r#""name":"Docs""#), "{}", request.body);
        assert!(
            request.body.contains(r#""mimeType":"application/vnd.google-apps.folder""#),
            "{}",
            request.body
        );
        assert!(request.body.contains(r#""parents":["parent-1"]"#), "{}", request.body);
    }

    #[tokio::test]
    async fn create_folder_omits_parents_at_the_root() {
        let stub = Stub::start(vec![Reply::json(200, r#"{"id":"root-1"}"#)]).await;
        let drive = drive(&stub);

        assert_eq!(drive.create_folder(ROOT, "Top").await.expect("create"), "root-1");

        let request = &stub.requests()[0];
        assert!(request.body.contains(r#""name":"Top""#), "{}", request.body);
        assert!(!request.body.contains("parents"), "{}", request.body);
    }

    const PAGE_ONE: &str = r#"{
        "nextPageToken": "page-2",
        "files": [
            {
                "id": "f1",
                "name": "notes.txt",
                "mimeType": "text/plain",
                "sizeBytes": "12",
                "md5Checksum": "0123456789abcdef0123456789abcdef",
                "parents": ["folder-1"],
                "trashed": false,
                "modifiedTime": "2026-01-02T03:04:05.000Z",
                "capabilities": {"canDownload": true}
            },
            {
                "id": "f2",
                "name": "Work",
                "mimeType": "application/vnd.google-apps.folder",
                "parents": ["folder-1"],
                "trashed": false,
                "capabilities": {"canDownload": false}
            }
        ]
    }"#;

    #[tokio::test]
    async fn list_folder_maps_one_page_and_hands_the_token_back() {
        let stub =
            Stub::start(vec![Reply::json(200, PAGE_ONE), Reply::json(200, r#"{"files":[]}"#)])
                .await;
        let drive = drive(&stub);

        let first = drive.list_folder("folder-1").await.expect("list");

        assert_eq!(first.next_page_token.as_deref(), Some("page-2"));
        assert_eq!(first.entries.len(), 2);
        let file = &first.entries[0];
        assert_eq!(file.id, "f1");
        assert_eq!(file.name, "notes.txt");
        assert_eq!(file.size_bytes, Some(12));
        assert_eq!(file.parent_id.as_deref(), Some("folder-1"));
        assert!(file.can_download);
        assert_eq!(file.modified_at.as_deref(), Some("2026-01-02T03:04:05.000Z"));
        // A folder carries no size and no download capability.
        assert_eq!(first.entries[1].mime_type, FOLDER_MIME);
        assert_eq!(first.entries[1].size_bytes, None);
        assert!(!first.entries[1].can_download);

        let request = &stub.requests()[0];
        assert_eq!(request.method, "GET");
        assert_eq!(
            request.query("q").as_deref(),
            Some("'folder-1' in parents and trashed = false")
        );
        assert_eq!(
            request.query("fields").as_deref(),
            Some(format!("nextPageToken,files({FILE_FIELDS})")).as_deref()
        );
        assert_eq!(request.query("pageSize").as_deref(), Some("100"));
        assert_eq!(request.query("pageToken").as_deref(), None);

        let second = drive
            .list_page("folder-1", 50, first.next_page_token.as_deref())
            .await
            .expect("page 2");
        assert_eq!(second.entries, vec![]);
        assert_eq!(second.next_page_token, None);
        assert_eq!(stub.requests()[1].query("pageToken").as_deref(), Some("page-2"));
        assert_eq!(stub.requests()[1].query("pageSize").as_deref(), Some("50"));
    }

    #[tokio::test]
    async fn listing_a_folder_that_does_not_exist_is_not_found() {
        let stub = Stub::start(vec![Reply::json(
            404,
            r#"{"error":{"code":404,"message":"File not found: 'gone'.","errors":[{"reason":"notFound"}]}}"#,
        )])
        .await;
        let drive = drive(&stub);

        let err = drive.list_folder("gone").await.expect_err("must fail");

        assert_eq!(err.code(), "not_found");
        // A 404 is not retried by the client.
        assert_eq!(stub.request_count(), 1);
    }
}
