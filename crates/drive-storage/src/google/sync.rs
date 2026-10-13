//! `about.get` for the account card and the quota, and `changes.list`, the feed F4 syncs.
//!
//! Every call returns exactly one page: the sync layer owns paging so it can checkpoint
//! between pages and never skip a change by replaying from a stale token.

use serde::Deserialize;

use super::GoogleDrive;
use super::map::{DriveFile, FILE_FIELDS, remote_file};
use crate::error::{ProviderError, Result};
use crate::types::{AccountInfo, ChangePage, Quota};

/// Drive's own ceiling for `changes.list`; a larger `pageSize` is rejected, not clamped. The
/// largest page also means one checkpoint per thousand changes instead of one per hundred.
const PAGE_SIZE: &str = "1000";

impl GoogleDrive {
    /// `fields=user` only, so no scope beyond `drive` is needed.
    pub async fn account_info(&self) -> Result<AccountInfo> {
        let about: AboutUser = self.client.get_json("about", &[("fields", "user")]).await?;
        let user = about.user;
        Ok(AccountInfo {
            email: user.email,
            name: user.display_name,
            account_id: user.permission_id,
        })
    }

    pub async fn quota(&self) -> Result<Quota> {
        let about: AboutQuota =
            self.client.get_json("about", &[("fields", "storageQuota")]).await?;
        let quota = about.storage_quota;
        Ok(Quota {
            limit_bytes: byte_count("limit", &quota.limit)?,
            usage_bytes: byte_count("usage", &quota.usage)?,
            usage_in_drive: optional_byte_count("usageInDrive", quota.usage_in_drive.as_deref())?,
            usage_in_drive_trash: optional_byte_count(
                "usageInDriveTrash",
                quota.usage_in_drive_trash.as_deref(),
            )?,
        })
    }

    /// One page of the change feed. `page_token` is the caller's own cursor; the token to store
    /// once the run finishes comes back in `new_start_page_token` on the final page.
    pub async fn changes(&self, page_token: Option<&str>) -> Result<ChangePage> {
        let fields = change_fields();
        let mut query = vec![("fields", fields.as_str()), ("pageSize", PAGE_SIZE)];
        if let Some(token) = page_token {
            query.push(("pageToken", token));
        }

        let list: ChangeList = self.client.get_json("changes", &query).await?;
        let mut page = ChangePage {
            changes: Vec::with_capacity(list.changes.len()),
            deleted_ids: Vec::new(),
            next_page_token: list.next_page_token,
            has_more: list.has_more,
            new_start_page_token: list.new_start_page_token,
        };

        for change in list.changes {
            if change.removed {
                // A removal carries no file resource, only the id the mirror has to forget.
                if let Some(id) = change.file_id {
                    page.deleted_ids.push(id);
                }
                continue;
            }
            // Drive omits `name` on the bare file resources it emits for permission and label
            // updates, which the local mirror does not model; writing them would put an
            // empty-named row in the tree.
            // shortcut: a permission-only change is therefore invisible to the sync engine;
            // revisit when per-file ACLs are modelled.
            let Some(file) = change.file.filter(|file| file.name.is_some()) else { continue };
            page.changes.push(remote_file(&file));
        }

        Ok(page)
    }
}

/// `fields=` for `changes.list`. Built from [`FILE_FIELDS`] so the mask cannot drift from what
/// [`remote_file`] reads.
fn change_fields() -> String {
    format!("changes(removed,fileId,file({FILE_FIELDS})),nextPageToken,hasMore,newStartPageToken")
}

/// Drive sends every quota figure as a JSON **string**. A value that will not parse is a decode
/// error naming the field: reading it as 0 would tell the user their Drive is full.
///
/// shortcut: Drive reports `limit` as `"0"` for an account with unlimited storage, so a UI must
/// not render a 0 limit as "Drive is full"; the quota card has to ask for the `unlimited` flag
/// separately when it is built.
fn byte_count(field: &str, raw: &str) -> Result<u64> {
    raw.parse().map_err(|_| {
        ProviderError::Decode(format!("storageQuota.{field} is not a byte count: {raw:?}"))
    })
}

fn optional_byte_count(field: &str, raw: Option<&str>) -> Result<Option<u64>> {
    raw.map(|raw| byte_count(field, raw)).transpose()
}

#[derive(Deserialize)]
struct AboutUser {
    user: AboutUserFields,
}

/// `emailAddress` is required: without it there is no account to attribute files to, so a
/// response missing it fails to decode instead of yielding an empty address.
#[derive(Deserialize)]
struct AboutUserFields {
    #[serde(rename = "emailAddress")]
    email: String,
    #[serde(rename = "displayName")]
    display_name: Option<String>,
    #[serde(rename = "permissionId")]
    permission_id: Option<String>,
}

#[derive(Deserialize)]
struct AboutQuota {
    #[serde(rename = "storageQuota")]
    storage_quota: StorageQuota,
}

#[derive(Deserialize)]
struct StorageQuota {
    limit: String,
    usage: String,
    #[serde(rename = "usageInDrive")]
    usage_in_drive: Option<String>,
    #[serde(rename = "usageInDriveTrash")]
    usage_in_drive_trash: Option<String>,
}

#[derive(Deserialize)]
struct ChangeList {
    #[serde(default)]
    changes: Vec<DriveChange>,
    #[serde(rename = "nextPageToken")]
    next_page_token: Option<String>,
    #[serde(rename = "hasMore", default)]
    has_more: bool,
    #[serde(rename = "newStartPageToken")]
    new_start_page_token: Option<String>,
}

#[derive(Deserialize)]
struct DriveChange {
    #[serde(rename = "fileId")]
    file_id: Option<String>,
    file: Option<DriveFile>,
    #[serde(default)]
    removed: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::google::stub::{Reply, Stub};
    use crate::types::RemoteFile;

    const EMPTY_PAGE: &str = r#"{"changes":[]}"#;

    const MIDDLE_PAGE: &str = r#"{
        "changes":[
            {"removed":false,"file":{"id":"moved","name":"a.txt","mimeType":"text/plain",
             "parents":["p2"],"modifiedTime":"2026-01-02T03:04:05Z"}},
            {"removed":true,"fileId":"deleted-1"}
        ],
        "nextPageToken":"page-2",
        "hasMore":true
    }"#;

    const LAST_PAGE: &str = r#"{"changes":[],"hasMore":false,"newStartPageToken":"start-9"}"#;

    async fn drive(stub: &Stub) -> GoogleDrive {
        GoogleDrive::with_endpoints("token", &stub.base, &stub.base)
    }

    fn ids(files: &[RemoteFile]) -> Vec<&str> {
        files.iter().map(|file| file.id.as_str()).collect()
    }

    #[tokio::test]
    async fn account_info_maps_the_user_resource() {
        let stub = Stub::start(vec![Reply::json(
            200,
            r#"{"user":{"emailAddress":"me@example.com","displayName":"Me","permissionId":"1234"}}"#,
        )])
        .await;

        let info = drive(&stub).await.account_info().await.expect("about.get");

        assert_eq!(info.email, "me@example.com");
        assert_eq!(info.name.as_deref(), Some("Me"));
        assert_eq!(info.account_id.as_deref(), Some("1234"));
        let request = &stub.requests()[0];
        assert_eq!(request.path, "/about");
        assert_eq!(request.query("fields").as_deref(), Some("user"));
    }

    #[tokio::test]
    async fn an_about_user_without_an_email_address_is_a_decode_error() {
        let stub = Stub::start(vec![Reply::json(200, r#"{"user":{"displayName":"Me"}}"#)]).await;

        let err = drive(&stub).await.account_info().await.expect_err("must fail");

        assert_eq!(err.code(), "decode");
        assert!(err.to_string().contains("missing field"), "{err}");
    }

    #[tokio::test]
    async fn quota_reads_the_string_encoded_byte_counts() {
        let stub = Stub::start(vec![Reply::json(
            200,
            r#"{"storageQuota":{"limit":"1099511627776","usage":"12","usageInDrive":"7",
                "usageInDriveTrash":"5"}}"#,
        )])
        .await;

        let quota = drive(&stub).await.quota().await.expect("about.get");

        assert_eq!(
            quota,
            Quota {
                limit_bytes: 1_099_511_627_776,
                usage_bytes: 12,
                usage_in_drive: Some(7),
                usage_in_drive_trash: Some(5),
            }
        );
        let request = &stub.requests()[0];
        assert_eq!(request.path, "/about");
        assert_eq!(request.query("fields").as_deref(), Some("storageQuota"));
    }

    #[tokio::test]
    async fn an_unparsable_limit_is_an_error_and_never_a_zero_reading() {
        let stub = Stub::start(vec![Reply::json(
            200,
            r#"{"storageQuota":{"limit":"15 TB","usage":"3"}}"#,
        )])
        .await;

        let err = drive(&stub).await.quota().await.expect_err("must fail");

        assert_eq!(err.code(), "decode");
        assert!(err.to_string().contains("limit"), "{err}");
    }

    #[tokio::test]
    async fn quota_leaves_the_optional_usage_fields_alone() {
        let stub =
            Stub::start(vec![Reply::json(200, r#"{"storageQuota":{"limit":"10","usage":"0"}}"#)])
                .await;

        let quota = drive(&stub).await.quota().await.expect("about.get");

        assert_eq!(quota.usage_in_drive, None);
        assert_eq!(quota.usage_in_drive_trash, None);
    }

    #[tokio::test]
    async fn changes_sends_the_page_token_only_when_the_caller_supplied_one() {
        let stub = Stub::start(vec![Reply::json(200, EMPTY_PAGE)]).await;
        let drive = drive(&stub).await;

        drive.changes(Some("page-2")).await.expect("page two");
        drive.changes(None).await.expect("first page");

        let requests = stub.requests();
        assert_eq!(requests[0].query("pageToken").as_deref(), Some("page-2"));
        assert_eq!(requests[1].query("pageToken"), None);
        assert_eq!(requests[1].query("pageSize").as_deref(), Some("1000"));
        let fields = requests[1].query("fields").expect("fields mask");
        assert!(fields.starts_with("changes(removed,fileId,file("), "{fields}");
        assert!(fields.contains(FILE_FIELDS), "{fields}");
        assert!(fields.ends_with("),nextPageToken,hasMore,newStartPageToken"), "{fields}");
    }

    #[tokio::test]
    async fn changes_separates_a_removal_from_a_trashed_file() {
        let stub = Stub::start(vec![Reply::json(
            200,
            r#"{"changes":[
                {"removed":true,"fileId":"gone"},
                {"removed":false,"file":{"id":"binned","name":"old.txt","mimeType":"text/plain",
                 "trashed":true}},
                {"removed":false,"file":{"id":"kept","name":"new.txt","mimeType":"text/plain"}}
            ]}"#,
        )])
        .await;

        let page = drive(&stub).await.changes(None).await.expect("changes.list");

        // `file.trashed` is a real change, so it lands in `changes` and not in `deleted_ids`.
        assert_eq!(page.deleted_ids, ["gone"]);
        assert_eq!(ids(&page.changes), ["binned", "kept"]);
        assert!(page.changes[0].trashed);
        assert!(!page.changes[1].trashed);
    }

    #[tokio::test]
    async fn a_change_whose_file_has_no_name_is_skipped_and_the_page_still_lands() {
        let stub = Stub::start(vec![Reply::json(
            200,
            r#"{"changes":[
                {"removed":false,"file":{"id":"perms-only","mimeType":"application/vnd.google-apps.document"}},
                {"removed":false,"file":{"id":"real","name":"a.txt","mimeType":"text/plain"}}
            ]}"#,
        )])
        .await;

        let page = drive(&stub).await.changes(None).await.expect("changes.list");

        assert_eq!(ids(&page.changes), ["real"]);
        assert!(page.deleted_ids.is_empty());
    }

    #[tokio::test]
    async fn a_removed_change_without_a_file_id_contributes_nothing() {
        let stub = Stub::start(vec![Reply::json(200, r#"{"changes":[{"removed":true}]}"#)]).await;

        let page = drive(&stub).await.changes(None).await.expect("changes.list");

        assert!(page.changes.is_empty());
        assert!(page.deleted_ids.is_empty());
    }

    #[tokio::test]
    async fn the_cursor_maps_through_and_only_the_last_page_invents_a_start_token() {
        let stub =
            Stub::start(vec![Reply::json(200, MIDDLE_PAGE), Reply::json(200, LAST_PAGE)]).await;
        let drive = drive(&stub).await;

        let middle = drive.changes(None).await.expect("middle page");
        assert!(middle.has_more);
        assert_eq!(middle.next_page_token.as_deref(), Some("page-2"));
        assert_eq!(middle.new_start_page_token, None);
        assert_eq!(ids(&middle.changes), ["moved"]);
        assert_eq!(middle.changes[0].parent_id.as_deref(), Some("p2"));
        assert_eq!(middle.changes[0].modified_at.as_deref(), Some("2026-01-02T03:04:05Z"));
        assert_eq!(middle.deleted_ids, ["deleted-1"]);

        let last = drive.changes(Some("page-2")).await.expect("last page");
        assert!(!last.has_more);
        assert_eq!(last.next_page_token, None);
        assert_eq!(last.new_start_page_token.as_deref(), Some("start-9"));
    }

    #[tokio::test]
    async fn changes_never_follows_the_next_page_token_itself() {
        let stub = Stub::start(vec![Reply::json(200, MIDDLE_PAGE)]).await;

        drive(&stub).await.changes(None).await.expect("one page");

        assert_eq!(stub.request_count(), 1);
    }

    #[tokio::test]
    async fn a_drive_error_envelope_surfaces_as_forbidden() {
        let stub = Stub::start(vec![Reply::json(
            403,
            r#"{"error":{"code":403,"message":"Rate Limit Exceeded"}}"#,
        )])
        .await;

        let err = drive(&stub).await.changes(None).await.expect_err("must fail");

        assert_eq!(err.code(), "forbidden");
        assert_eq!(stub.request_count(), 1);
    }
}
