//! The sync engine: folds a provider's change feed into the local mirror.
//!
//! F4 of `docs/rewrite-plan.md` §8. Drive's `9drive` folder is the source of truth; rows here
//! are a mirror, so anything the feed reports as gone is marked trashed rather than deleted
//! outright.

use drive_storage::provider::StorageProvider;

use crate::db::{SharedConn, with_conn};
use crate::error::{AppError, Result};
use crate::progress::{ProgressEvent, ProgressSink, SyncState, report};
use crate::repos::{accounts, files, sync_state};

/// The one Drive folder this app owns, and the name it is looked up by.
const APP_FOLDER: &str = "9drive";

/// What one account's sync did. Returned for logging and for the F5 command that reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncOutcome {
    /// Change entries applied, upserts and deletions together (`progress::Sync::processed`
    /// counts the same thing).
    pub processed: u64,
    pub upserted: u64,
    pub deleted: u64,
    /// False when the run restarted from scratch because the stored token had expired.
    pub incremental: bool,
}

/// Folds one account's change feed into the local mirror, one page at a time.
///
/// Retries live in the Drive HTTP client, which already backs off on 429, rate-limit 403 and
/// 5xx; a loop here would multiply its attempts and turn a slow failure into a hang.
pub async fn sync_account(
    conn: &SharedConn,
    provider: &dyn StorageProvider,
    account_id: i64,
    progress: Option<&ProgressSink>,
) -> Result<SyncOutcome> {
    report(progress, ProgressEvent::Sync { account_id, processed: 0, state: SyncState::Started });

    // One restart, never a retry loop: a page token expires about once every 30 days, so a
    // second `invalidPageToken` is some other fault and re-scanning the user's whole Drive
    // again would only spin.
    let mut restarted = false;
    let mut outcome = loop {
        match drain(conn, provider, account_id, progress).await {
            Ok(outcome) => break outcome,
            Err(err) if !restarted && is_expired_token(&err) => {
                restarted = true;
                report(
                    progress,
                    ProgressEvent::Sync { account_id, processed: 0, state: SyncState::Restarted },
                );
                // The next run reads the stored token, so the dead one has to go first.
                with_conn(conn, |c| sync_state::upsert(c, account_id, None))?;
            }
            Err(err) => return Err(err),
        }
    };
    if restarted {
        outcome.incremental = false;
    }
    report(
        progress,
        ProgressEvent::Sync {
            account_id,
            processed: outcome.processed,
            state: SyncState::Finished,
        },
    );
    Ok(outcome)
}

/// One full pass over the feed, from the stored token to the last page.
async fn drain(
    conn: &SharedConn,
    provider: &dyn StorageProvider,
    account_id: i64,
    progress: Option<&ProgressSink>,
) -> Result<SyncOutcome> {
    // `accounts::get` already answers `NotFound`, so a vanished account needs no own check.
    with_conn(conn, |c| accounts::get(c, account_id).map(|_| ()))?;

    // Called on every run, even when the row already carries an id: it is idempotent by name
    // and is the only thing that repairs a folder the user deleted on Drive's side.
    let folder_id = provider.ensure_app_folder(APP_FOLDER).await?;
    with_conn(conn, |c| accounts::set_drive_folder_id(c, account_id, &folder_id))?;

    let mut token = with_conn(conn, |c| match sync_state::get(c, account_id) {
        Ok(state) => Ok(state.start_page_token),
        // No row: this account has never synced, so the feed starts at its first page.
        Err(AppError::NotFound { .. }) => Ok(None),
        Err(err) => Err(err),
    })?;

    // shortcut: accounts run one at a time; how many to run at once belongs to the F5 command,
    // and a task set here would only move that decision without answering it.
    let mut outcome = SyncOutcome { processed: 0, upserted: 0, deleted: 0, incremental: true };
    loop {
        let page = provider.changes(token.as_deref()).await?;
        // `processed` counts the feed's entries, so a deletion already mirrored locally still
        // counts as applied; `deleted` counts the rows that actually changed.
        let upserted = page.changes.len() as u64;
        let entries = upserted + page.deleted_ids.len() as u64;
        // One lock scope for the whole page, and it holds no guard across the awaits above.
        let deleted = with_conn(conn, |c| {
            for change in &page.changes {
                files::upsert_remote(c, account_id, change)?;
            }
            Ok(files::mark_deleted(c, account_id, &page.deleted_ids)? as u64)
        })?;
        outcome.upserted += upserted;
        outcome.deleted += deleted;
        outcome.processed += entries;
        report(
            progress,
            ProgressEvent::Sync {
                account_id,
                processed: outcome.processed,
                state: SyncState::Running,
            },
        );

        // Only the last page carries this token; without storing it the next run replays the
        // whole window. `sync_state::upsert` stamps `last_sync_at` in the same write.
        if let Some(new_token) = page.new_start_page_token {
            with_conn(conn, |c| sync_state::upsert(c, account_id, Some(&new_token)))?;
            break;
        }
        // An empty page is not the end of the run: the Drive layer drops permission and label
        // changes that carry no `name`, so a page can model nothing while later pages still
        // carry real work. Breaking here would silently truncate the sync.
        let Some(next) = page.next_page_token.filter(|next| !next.is_empty()) else { break };
        // A token repeating the one just used would page the feed forever; one comparison is
        // cheaper than a hung UI.
        if token.as_deref() == Some(next.as_str()) {
            break;
        }
        token = Some(next);
    }
    Ok(outcome)
}

/// Drive rejects a page token older than 30 days with `400 invalidPageToken`. The storage
/// layer maps every 400 to `ProviderError::Provider` with the reason in the message, so the
/// message is the only place the expired token can be recognised.
///
/// `shortcut:` matching Google's wording means a rewording silently breaks the restart and
/// sync fails until the account's stored token is cleared by hand. Add a
/// `ProviderError::PageTokenExpired` variant mapped from the `errors[].reason` when that
/// starts happening.
fn is_expired_token(err: &AppError) -> bool {
    let AppError::Storage(provider_err) = err else { return false };
    provider_err.to_string().to_lowercase().contains("invalidpagetoken")
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use drive_storage::error::{ProviderError, Result as ProviderResult};
    use drive_storage::provider::UploadProgress;
    use drive_storage::types::{
        AccountInfo, ChangePage, FolderListing, ProviderKind, Quota, Range, RemoteFile,
        UploadRequest,
    };

    use super::*;
    use crate::db::{open_in_memory, shared};
    use crate::repos::files::FileFilter;

    type StubPage = std::result::Result<ChangePage, ProviderError>;

    /// Canned pages popped one per `changes` call, plus the exact sequence of tokens the run
    /// asked for. An empty queue is an error rather than a panic, so a run that fails to
    /// terminate fails its test instead of hanging the suite.
    struct StubProvider {
        folder_id: String,
        pages: Mutex<VecDeque<StubPage>>,
        requested: Mutex<Vec<Option<String>>>,
    }

    impl StubProvider {
        fn new(pages: Vec<StubPage>) -> Self {
            Self {
                folder_id: "folder-9drive".into(),
                pages: Mutex::new(pages.into()),
                requested: Mutex::new(Vec::new()),
            }
        }

        fn requested(&self) -> Vec<Option<String>> {
            self.requested.lock().unwrap().clone()
        }
    }

    /// Every method the sync engine has no business calling says so by name if it ever does.
    fn unused(method: &str) -> ! {
        panic!("StorageProvider::{method} must not be called by the sync engine")
    }

    #[async_trait]
    impl StorageProvider for StubProvider {
        fn kind(&self) -> ProviderKind {
            ProviderKind::Google
        }
        async fn ensure_app_folder(&self, _name: &str) -> ProviderResult<String> {
            Ok(self.folder_id.clone())
        }
        async fn changes(&self, page_token: Option<&str>) -> ProviderResult<ChangePage> {
            self.requested.lock().unwrap().push(page_token.map(str::to_string));
            self.pages.lock().unwrap().pop_front().unwrap_or_else(|| {
                Err(ProviderError::Provider("stub out of pages: the run did not terminate".into()))
            })
        }
        async fn account_info(&self) -> ProviderResult<AccountInfo> {
            unused("account_info")
        }
        async fn quota(&self) -> ProviderResult<Quota> {
            unused("quota")
        }
        async fn create_folder(&self, _parent_id: &str, _name: &str) -> ProviderResult<String> {
            unused("create_folder")
        }
        async fn list_folder(&self, _folder_id: &str) -> ProviderResult<FolderListing> {
            unused("list_folder")
        }
        async fn upload(
            &self,
            _req: UploadRequest,
            _progress: Option<&UploadProgress>,
        ) -> ProviderResult<RemoteFile> {
            unused("upload")
        }
        async fn download(&self, _file_id: &str, _range: Option<Range>) -> ProviderResult<Vec<u8>> {
            unused("download")
        }
        async fn set_trashed(&self, _file_id: &str, _trashed: bool) -> ProviderResult<()> {
            unused("set_trashed")
        }
        async fn delete(&self, _file_id: &str) -> ProviderResult<()> {
            unused("delete")
        }
    }

    fn remote(id: &str, name: &str) -> RemoteFile {
        RemoteFile {
            id: id.into(),
            name: name.into(),
            mime_type: "text/plain".into(),
            size_bytes: Some(12),
            md5_checksum: None,
            parent_id: None,
            trashed: false,
            modified_at: None,
            can_download: true,
        }
    }

    fn page(
        changes: Vec<RemoteFile>,
        deleted_ids: Vec<&str>,
        next_page_token: Option<&str>,
        new_start_page_token: Option<&str>,
    ) -> ChangePage {
        ChangePage {
            changes,
            deleted_ids: deleted_ids.into_iter().map(str::to_string).collect(),
            next_page_token: next_page_token.map(str::to_string),
            has_more: next_page_token.is_some(),
            new_start_page_token: new_start_page_token.map(str::to_string),
        }
    }

    /// The shape `GoogleDrive::changes` hands back for Drive's `400 invalidPageToken`.
    fn expired_token() -> StubPage {
        Err(ProviderError::Provider(
            "400 Bad Request: invalidPageToken: Invalid Page Token.".into(),
        ))
    }

    fn setup() -> (SharedConn, i64) {
        let conn = shared(open_in_memory().unwrap());
        let account_id =
            with_conn(&conn, |c| Ok(accounts::create(c, "google", "User", "a@example.com")?.id))
                .unwrap();
        (conn, account_id)
    }

    fn new_account(conn: &SharedConn, email: &str) -> i64 {
        with_conn(conn, |c| Ok(accounts::create(c, "google", "User", email)?.id)).unwrap()
    }

    fn seed_token(conn: &SharedConn, account_id: i64, token: &str) {
        with_conn(conn, |c| sync_state::upsert(c, account_id, Some(token))).unwrap();
    }

    fn stored_token(conn: &SharedConn, account_id: i64) -> Option<String> {
        with_conn(conn, |c| Ok(sync_state::get(c, account_id)?.start_page_token)).unwrap()
    }

    /// Every row of an account, trashed or not, as `(drive_file_id, trashed)`.
    fn rows(conn: &SharedConn, account_id: i64) -> Vec<(String, bool)> {
        with_conn(conn, |c| {
            let mut all: Vec<(String, bool)> = files::list(c, &FileFilter::default())?
                .into_iter()
                .filter(|file| file.account_id == account_id)
                .map(|file| (file.drive_file_id, false))
                .collect();
            all.extend(
                files::list(c, &FileFilter { trashed: true, ..Default::default() })?
                    .into_iter()
                    .filter(|file| file.account_id == account_id)
                    .map(|file| (file.drive_file_id, true)),
            );
            all.sort();
            Ok(all)
        })
        .unwrap()
    }

    fn collector() -> (Arc<Mutex<Vec<ProgressEvent>>>, ProgressSink) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink_seen = Arc::clone(&seen);
        (seen, Box::new(move |event| sink_seen.lock().unwrap().push(event)))
    }

    /// A `Connection` held across an await makes the whole future non-`Send`, which only
    /// fails deep inside the caller's `tokio::spawn`; this asserts it at compile time instead.
    #[test]
    fn the_sync_future_is_send() {
        fn assert_send<T: Send>(value: T) {
            drop(value);
        }
        let (conn, account_id) = setup();
        let stub = StubProvider::new(vec![]);

        assert_send(sync_account(&conn, &stub, account_id, None));
    }

    #[tokio::test]
    async fn a_missing_account_is_reported_as_not_found() {
        let (conn, _) = setup();
        let stub = StubProvider::new(vec![]);

        let err = sync_account(&conn, &stub, 404, None).await.unwrap_err();

        assert!(matches!(err, AppError::NotFound { entity: "account", id: 404 }), "got {err:?}");
        assert!(stub.requested().is_empty(), "a missing account makes no feed call");
    }

    #[tokio::test]
    async fn a_single_page_upserts_every_entry_and_stores_the_new_start_page_token() {
        let (conn, account_id) = setup();
        seed_token(&conn, account_id, "stored");
        let stub = StubProvider::new(vec![Ok(page(
            vec![remote("d1", "a.txt"), remote("d2", "b.txt")],
            vec![],
            None,
            Some("fresh"),
        ))]);

        let outcome = sync_account(&conn, &stub, account_id, None).await.unwrap();

        assert_eq!(
            outcome,
            SyncOutcome { processed: 2, upserted: 2, deleted: 0, incremental: true }
        );
        assert_eq!(stub.requested(), [Some("stored".to_string())]);
        assert_eq!(rows(&conn, account_id), [("d1".into(), false), ("d2".into(), false)]);
        assert_eq!(stored_token(&conn, account_id).as_deref(), Some("fresh"));
    }

    #[tokio::test]
    async fn a_multi_page_run_is_drained_past_an_empty_page() {
        let (conn, account_id) = setup();
        seed_token(&conn, account_id, "stored");
        let stub = StubProvider::new(vec![
            Ok(page(vec![remote("d1", "a.txt")], vec![], Some("p2"), None)),
            // Nameless permission changes land here as an empty page, not as the end of the feed.
            Ok(page(vec![], vec![], Some("p3"), None)),
            Ok(page(vec![remote("d2", "b.txt")], vec![], None, Some("fresh"))),
        ]);

        let outcome = sync_account(&conn, &stub, account_id, None).await.unwrap();

        assert_eq!(
            stub.requested(),
            [Some("stored".into()), Some("p2".into()), Some("p3".into())],
            "the empty middle page must not end the run"
        );
        assert_eq!(outcome.upserted, 2, "the last page's entry still has to land: {outcome:?}");
        assert_eq!(rows(&conn, account_id), [("d1".into(), false), ("d2".into(), false)]);
        assert_eq!(stored_token(&conn, account_id).as_deref(), Some("fresh"));
    }

    #[tokio::test]
    async fn mark_deleted_trashes_this_accounts_rows_only() {
        let (conn, account_id) = setup();
        let other = new_account(&conn, "b@example.com");
        seed_token(&conn, account_id, "stored");
        with_conn(&conn, |c| {
            for (id, drive_file_id) in [(account_id, "gone-a"), (other, "gone-b")] {
                files::upsert_remote(c, id, &remote(drive_file_id, "gone.txt"))?;
                // `mark_deleted` ignores rows written within the last minute, so both start aged.
                c.execute(
                    "UPDATE files SET updated_at = 1 WHERE drive_file_id = ?1",
                    [drive_file_id],
                )?;
            }
            Ok(())
        })
        .unwrap();
        let stub = StubProvider::new(vec![Ok(page(
            vec![],
            vec!["gone-a", "gone-b"],
            None,
            Some("fresh"),
        ))]);

        let outcome = sync_account(&conn, &stub, account_id, None).await.unwrap();

        assert_eq!(outcome.deleted, 1);
        assert_eq!(outcome.processed, 2, "a deletion counts as an applied change entry");
        assert_eq!(rows(&conn, account_id), [("gone-a".into(), true)]);
        assert_eq!(
            rows(&conn, other),
            [("gone-b".into(), false)],
            "another account's row is untouched"
        );
    }

    #[tokio::test]
    async fn a_page_without_a_new_start_page_token_leaves_the_stored_token_alone() {
        let (conn, account_id) = setup();
        seed_token(&conn, account_id, "stored");
        let stub =
            StubProvider::new(vec![Ok(page(vec![remote("d1", "a.txt")], vec![], None, None))]);

        let outcome = sync_account(&conn, &stub, account_id, None).await.unwrap();

        assert_eq!(outcome.upserted, 1);
        assert_eq!(stored_token(&conn, account_id).as_deref(), Some("stored"));
    }

    #[tokio::test]
    async fn an_empty_next_page_token_ends_the_run() {
        let (conn, account_id) = setup();
        seed_token(&conn, account_id, "stored");
        let stub =
            StubProvider::new(vec![Ok(page(vec![remote("d1", "a.txt")], vec![], Some(""), None))]);

        let outcome = sync_account(&conn, &stub, account_id, None).await.unwrap();

        assert_eq!(outcome.upserted, 1);
        assert_eq!(stub.requested().len(), 1);
    }

    #[tokio::test]
    async fn a_next_page_token_repeating_the_one_in_use_ends_the_run() {
        let (conn, account_id) = setup();
        seed_token(&conn, account_id, "loop");
        let stub = StubProvider::new(vec![Ok(page(
            vec![remote("d1", "a.txt")],
            vec![],
            Some("loop"),
            None,
        ))]);

        let outcome = sync_account(&conn, &stub, account_id, None).await.unwrap();

        assert_eq!(outcome.upserted, 1);
        assert_eq!(stub.requested().len(), 1, "the same page must not be asked for twice");
        assert_eq!(stored_token(&conn, account_id).as_deref(), Some("loop"));
    }

    #[tokio::test]
    async fn an_expired_page_token_restarts_the_run_once_and_says_so() {
        let (conn, account_id) = setup();
        seed_token(&conn, account_id, "expired");
        let stub = StubProvider::new(vec![
            expired_token(),
            Ok(page(vec![remote("d1", "a.txt")], vec![], None, Some("fresh"))),
        ]);
        let (seen, sink) = collector();

        let outcome = sync_account(&conn, &stub, account_id, Some(&sink)).await.unwrap();

        assert!(!outcome.incremental, "a restart re-scans the whole Drive");
        assert_eq!(
            stub.requested(),
            [Some("expired".into()), None],
            "the restart starts from the top of the feed"
        );
        assert_eq!(stored_token(&conn, account_id).as_deref(), Some("fresh"));
        assert_eq!(rows(&conn, account_id), [("d1".into(), false)]);

        let seen = seen.lock().unwrap();
        assert!(
            seen.iter().any(|event| matches!(
                event,
                ProgressEvent::Sync { state: SyncState::Restarted, .. }
            )),
            "a full re-scan must be visible: {seen:?}"
        );
        assert!(matches!(
            seen.first(),
            Some(ProgressEvent::Sync { state: SyncState::Started, .. })
        ));
        assert!(matches!(
            seen.last(),
            Some(ProgressEvent::Sync { state: SyncState::Finished, .. })
        ));
    }

    #[tokio::test]
    async fn a_second_expired_page_token_is_an_error_rather_than_a_loop() {
        let (conn, account_id) = setup();
        seed_token(&conn, account_id, "expired");
        let stub = StubProvider::new(vec![expired_token(), expired_token()]);

        let err = sync_account(&conn, &stub, account_id, None).await.unwrap_err();

        assert!(matches!(err, AppError::Storage(_)), "got {err:?}");
        assert_eq!(stub.requested().len(), 2, "exactly one restart, then give up");
        assert_eq!(stored_token(&conn, account_id), None, "the dead token stays dropped");
    }

    #[tokio::test]
    async fn ensure_app_folder_runs_every_time_and_its_id_lands_on_the_account() {
        let (conn, account_id) = setup();
        // A folder deleted on Drive's side leaves a stale id behind; only the idempotent
        // lookup by name can repair it.
        with_conn(&conn, |c| {
            c.execute("UPDATE accounts SET drive_folder_id = 'stale' WHERE id = ?1", [account_id])?;
            Ok(())
        })
        .unwrap();
        let stub = StubProvider::new(vec![Ok(page(vec![], vec![], None, Some("fresh")))]);

        sync_account(&conn, &stub, account_id, None).await.unwrap();

        let folder_id =
            with_conn(&conn, |c| Ok(accounts::get(c, account_id)?.drive_folder_id)).unwrap();
        assert_eq!(folder_id.as_deref(), Some("folder-9drive"));
    }

    #[tokio::test]
    async fn an_uncached_account_starts_the_feed_from_its_first_page() {
        let (conn, account_id) = setup();
        let stub = StubProvider::new(vec![Ok(page(
            vec![remote("d1", "a.txt")],
            vec![],
            None,
            Some("fresh"),
        ))]);

        let outcome = sync_account(&conn, &stub, account_id, None).await.unwrap();

        assert_eq!(stub.requested(), [None], "no token row yet means no page token");
        assert_eq!(outcome.upserted, 1);
        assert_eq!(stored_token(&conn, account_id).as_deref(), Some("fresh"));
    }
}
