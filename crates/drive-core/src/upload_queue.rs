//! The upload queue: concurrency, pause/resume, cancel, retry and per-file progress.
//!
//! F4 of `docs/rewrite-plan.md` §8. Routing decides the account, the provider does the bytes,
//! and this module is what the Tauri command holds across the whole operation.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use drive_storage::provider::StorageProvider;
use drive_storage::types::UploadRequest;
use tokio::sync::Notify;
use tokio::task::JoinSet;

use crate::db::{SharedConn, now_unix, with_conn};
use crate::error::{AppError, Result};
use crate::import::ImportCandidate;
use crate::progress::{ProgressEvent, ProgressSink, UploadState, report};
use crate::repos::{accounts, files};
use crate::routing::Router;

/// One file waiting to be uploaded. No database row: local rows mirror Drive, so a row does not
/// exist until the upload succeeds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadTask {
    /// Assigned by the queue, never by the caller. `0` means "not assigned yet".
    pub task_id: i64,
    pub source_path: PathBuf,
    pub name: String,
    pub mime_type: String,
    pub size_bytes: u64,
    /// Virtual folder to file the upload under once the row exists. `None` leaves it at the
    /// Drive root, which is what a sync would see anyway.
    pub folder_id: Option<i64>,
}

#[derive(Debug, Clone, Copy)]
pub struct QueueConfig {
    /// How many files upload at once.
    pub concurrency: usize,
    /// Total attempts per file, the first one included.
    pub max_attempts: u32,
}

impl Default for QueueConfig {
    fn default() -> Self {
        Self { concurrency: 3, max_attempts: 3 }
    }
}

/// What one batch did. A file that failed is data, not an error: the caller still wants the ones
/// that landed.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UploadReport {
    /// `(task_id, files.id)` per success.
    pub finished: Vec<(i64, i64)>,
    /// `(task_id, AppError::code())` per file that ran out of attempts.
    pub failed: Vec<(i64, String)>,
}

/// `account_id` for a task the router never picked an account for. No account row carries this
/// id, so the frontend can tell "not chosen yet" from a real one; a task that did get an account
/// is already showing it from its `Started` event.
const UNROUTED: i64 = 0;

/// Holds the flags F5 flips from another command while `enqueue` is still running, which is why
/// they are interior mutability rather than `&mut self`.
pub struct UploadQueue {
    /// `select` needs `&mut Router` and is synchronous, so a std mutex taken around that one
    /// call is enough: it is never held across the upload await.
    router: Arc<Mutex<Router>>,
    config: QueueConfig,
    paused: AtomicBool,
    cancelled: AtomicBool,
    /// Wakes the dispatcher. A permit is stored when nothing is parked yet, so a `resume` that
    /// races the flag check cannot be lost.
    resumed: Notify,
}

impl UploadQueue {
    pub fn new(router: Router, config: QueueConfig) -> Self {
        Self {
            router: Arc::new(Mutex::new(router)),
            config,
            paused: AtomicBool::new(false),
            cancelled: AtomicBool::new(false),
            resumed: Notify::new(),
        }
    }

    /// Uploads `tasks` through the router and the provider, at most `config.concurrency` at once.
    ///
    /// `Ok` even when files fail: one file that could not be uploaded is reported in
    /// [`UploadReport::failed`] and the rest still run. The `Err` is the queue itself failing --
    /// a spawned task panicking.
    pub async fn enqueue(
        &self,
        conn: &SharedConn,
        // `Arc` rather than `&dyn StorageProvider`: a spawned task has to be `'static`, and no
        // borrowed-future join is available without the `futures` crate.
        provider: Arc<dyn StorageProvider>,
        mut tasks: Vec<UploadTask>,
        progress: Option<Arc<ProgressSink>>,
    ) -> Result<UploadReport> {
        // `tasks_for` numbers a batch it built; a caller that assembled tasks itself leaves 0
        // for the queue to fill, so the ids in the events always match the report.
        for (index, task) in tasks.iter_mut().enumerate() {
            if task.task_id == 0 {
                task.task_id = index as i64 + 1;
            }
        }
        // The flags describe the run in progress and a queue runs one batch at a time, so a new
        // enqueue does not inherit the previous batch's cancel.
        self.paused.store(false, Ordering::SeqCst);
        self.cancelled.store(false, Ordering::SeqCst);

        let conn = Arc::clone(conn);
        let router = Arc::clone(&self.router);
        let mut report = UploadReport::default();
        let mut running = JoinSet::new();
        let mut next = 0;

        while next < tasks.len() || !running.is_empty() {
            // Both flags gate dispatch only, which is what "between files" means.
            if next < tasks.len() && self.cancelled.load(Ordering::SeqCst) {
                // shortcut: a file already uploading runs to completion, because
                // `StorageProvider::upload` is one await with no token for the queue to observe;
                // cancelling inside a resumable upload needs the provider to check a token between
                // chunks, which is the chunk loop in `google/transfer.rs`.
                for task in &tasks[next..] {
                    emit(progress.as_deref(), task, UNROUTED, 0, UploadState::Cancelled);
                }
                next = tasks.len();
            } else if next < tasks.len() && self.paused.load(Ordering::SeqCst) {
                if let Some(task) = tasks.get(next) {
                    emit(progress.as_deref(), task, UNROUTED, 0, UploadState::Paused);
                }
                self.resumed.notified().await;
                continue;
            }
            while next < tasks.len() && running.len() < self.config.concurrency {
                let task = tasks[next].clone();
                next += 1;
                running.spawn(run_task(
                    Arc::clone(&conn),
                    Arc::clone(&provider),
                    Arc::clone(&router),
                    self.config,
                    task,
                    progress.clone(),
                ));
            }
            let Some(joined) = running.join_next().await else { continue };
            match joined {
                Ok(TaskOutcome::Finished { task_id, row_id }) => {
                    report.finished.push((task_id, row_id))
                }
                Ok(TaskOutcome::Failed { task_id, code }) => report.failed.push((task_id, code)),
                // A task that panicked leaves no outcome to attribute, and what ran around it
                // cannot be trusted, so the run stops rather than reporting a lie.
                Err(joined) => {
                    return Err(AppError::Conflict(format!("an upload task panicked: {joined}")));
                }
            }
        }
        Ok(report)
    }

    pub fn pause(&self) {
        self.paused.store(true, Ordering::SeqCst);
    }

    pub fn resume(&self) {
        self.paused.store(false, Ordering::SeqCst);
        self.resumed.notify_one();
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        self.resumed.notify_one();
    }
}

/// One file from routing to a mirrored row: pick an account, upload, retry, mirror.
async fn run_task(
    conn: SharedConn,
    provider: Arc<dyn StorageProvider>,
    router: Arc<Mutex<Router>>,
    config: QueueConfig,
    task: UploadTask,
    progress: Option<Arc<ProgressSink>>,
) -> TaskOutcome {
    let sink = progress.as_deref();
    // Shared with the progress callback, which owns it because `UploadProgress`'s trait object is
    // `'static` and so cannot borrow the bytes counter it is reporting.
    let high_water = Arc::new(AtomicU64::new(0));
    let attempts = config.max_attempts.max(1);
    let mut attempt = 0;
    // The loop runs at least once and every path leaves it by breaking, so there is no invented
    // error for a case that cannot happen.
    let outcome = loop {
        attempt += 1;
        // The router lock covers `select` and nothing else: concurrent uploads share one router
        // without queueing behind each other's bytes.
        let choice = match router
            .lock()
            .map_err(|_| AppError::Conflict("router lock poisoned by a panicking task".into()))
            .and_then(|mut router| router.select(&conn, task.size_bytes))
        {
            Ok(choice) => choice,
            Err(err) => break Err(err),
        };
        let account_id = choice.account.id;
        // The router skips accounts with no Drive folder, so an absent one means the row changed
        // between its read and now rather than anything the queue can upload into.
        let Some(parent_id) = choice.account.drive_folder_id.clone() else {
            break Err(AppError::Conflict(format!(
                "account {account_id} has no Drive folder to upload into"
            )));
        };
        // Once per task, not once per attempt: a retry that restarts at zero would send the UI a
        // byte count lower than the one it already showed.
        if attempt == 1 {
            emit(sink, &task, account_id, 0, UploadState::Started);
        }
        let request = UploadRequest {
            parent_id,
            name: task.name.clone(),
            mime_type: task.mime_type.clone(),
            size_bytes: task.size_bytes,
            source_path: task.source_path.to_string_lossy().into_owned(),
        };
        let on_progress = {
            // Everything the callback reports is copied in, because it is `'static` and cannot
            // borrow this loop's frame.
            let sink = progress.clone();
            let high_water = Arc::clone(&high_water);
            let (task_id, name, total) = (task.task_id, task.name.clone(), task.size_bytes);
            move |sent: u64| {
                let sent = high_water.fetch_max(sent, Ordering::SeqCst);
                report(
                    sink.as_deref(),
                    event(task_id, &name, account_id, sent, Some(total), UploadState::Running),
                );
            }
        };
        // Re-routed on every attempt: an account that was full a moment ago may have room now,
        // which is the whole reason the queue holds a router rather than one account per task.
        match provider.upload(request, Some(&on_progress)).await {
            Ok(remote) => break Ok((choice.account, remote)),
            // Keep the attempt going while there is budget left; the last failure is the one
            // reported, never a repeat of it.
            Err(_err) if attempt < attempts => continue,
            // Provider codes are already the contract the frontend switches on.
            Err(err) => break Err(AppError::Storage(err)),
        }
    };

    match outcome {
        Ok((account, remote)) => {
            // One lock scope for both writes, and no guard is held across the await above.
            let mirrored = with_conn(&conn, |c| {
                let row = files::upsert_remote(c, account.id, &remote)?;
                // The mirror writes `folder_id` as NULL on every sync, so the folder the user
                // chose has to be re-applied here or the upload lands at the Drive root.
                files::move_to_folder(c, row.id, task.folder_id)?;
                // routing.rs leaves this stamp to the queue; without it `last_used_at` stays NULL
                // forever and "recently used" never means anything.
                accounts::update(c, account.id, &account.name, Some(now_unix()))?;
                Ok(row)
            });
            match mirrored {
                Ok(row) => {
                    let sent = high_water.load(Ordering::SeqCst);
                    emit(sink, &task, account.id, sent, UploadState::Finished);
                    TaskOutcome::Finished { task_id: task.task_id, row_id: row.id }
                }
                Err(err) => failed(&task, sink, err.code()),
            }
        }
        Err(err) => failed(&task, sink, err.code()),
    }
}

/// What one finished task contributed to the batch report.
enum TaskOutcome {
    Finished { task_id: i64, row_id: i64 },
    Failed { task_id: i64, code: String },
}

/// The one terminal failure path: the code goes to the UI, never a provider message.
fn failed(task: &UploadTask, sink: Option<&ProgressSink>, code: &str) -> TaskOutcome {
    emit(sink, task, UNROUTED, 0, UploadState::Failed);
    TaskOutcome::Failed { task_id: task.task_id, code: code.to_string() }
}

fn emit(
    progress: Option<&ProgressSink>,
    task: &UploadTask,
    account_id: i64,
    sent: u64,
    state: UploadState,
) {
    report(
        progress,
        event(task.task_id, &task.name, account_id, sent, Some(task.size_bytes), state),
    );
}

/// One upload tick. Takes the name by reference so the provider's callback can build one from the
/// copy it owns, without borrowing the task.
fn event(
    task_id: i64,
    name: &str,
    account_id: i64,
    sent: u64,
    total: Option<u64>,
    state: UploadState,
) -> ProgressEvent {
    ProgressEvent::Upload { task_id, name: name.to_string(), account_id, sent, total, state }
}

/// Numbers a batch of candidates in the order `import::expand` returned them, and files them
/// under `folder_id`. The Drive parent of an upload is the `drive_folder_id` of whichever
/// account routing picked; a virtual folder is only the local row the mirror lands in.
pub fn tasks_for(
    candidates: Vec<ImportCandidate>,
    folder_id: Option<i64>,
) -> Result<Vec<UploadTask>> {
    Ok(candidates
        .into_iter()
        .enumerate()
        .map(|(index, candidate)| UploadTask {
            task_id: index as i64 + 1,
            folder_id,
            ..candidate.into()
        })
        .collect())
}

// shortcut: the formats Drive previews inline, and nothing else; add entries as F6 shows which
// previews matter. Every other extension is uploaded as `application/octet-stream`.
pub fn mime_for(path: &Path) -> &'static str {
    let extension = path.extension().and_then(|ext| ext.to_str()).map(str::to_ascii_lowercase);
    match extension.as_deref() {
        Some("pdf") => "application/pdf",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("svg") => "image/svg+xml",
        Some("heic") => "image/heic",
        Some("txt" | "md" | "csv") => "text/plain",
        Some("json") => "application/json",
        Some("zip") => "application/zip",
        Some("mp4") => "video/mp4",
        Some("webm") => "video/webm",
        Some("mov") => "video/quicktime",
        Some("mp3") => "audio/mpeg",
        Some("m4a") => "audio/mp4",
        Some("wav") => "audio/wav",
        Some("ogg") => "audio/ogg",
        Some("flac") => "audio/flac",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::sync::atomic::AtomicUsize;
    use std::time::Duration;

    use async_trait::async_trait;
    use drive_storage::error::ProviderError;
    use drive_storage::provider::UploadProgress;
    use drive_storage::types::{
        AccountInfo, ChangePage, FolderListing, ProviderKind, Quota, Range, RemoteFile,
    };
    use rusqlite::params;

    use crate::db::{open_in_memory, shared};
    use crate::import;
    use crate::models::AccountQuota;
    use crate::repos::files::FileFilter;
    use crate::repos::folders;
    use crate::routing::Policy;

    const GB: i64 = 1_000_000_000;
    const PAYLOAD: &[u8] = b"payload";

    /// Answers from a script instead of a network, and always terminates: no clock, no retries
    /// of its own, so a queue bug shows up as a failing assertion and not as a hung test.
    #[derive(Default)]
    struct Stub {
        /// Shared, so a test can read the count after the stub has been moved into the queue.
        calls: Arc<AtomicUsize>,
        /// Refuse the first N uploads, so a test can prove the queue retries them.
        fail_first: usize,
        /// Refuse every upload of this name, however many attempts are made.
        refuse: Option<String>,
        /// Runs on the first upload only, so a test can cancel, pause or fill an account.
        on_first_upload: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
    }

    #[async_trait]
    impl StorageProvider for Stub {
        fn kind(&self) -> ProviderKind {
            panic!("stub provider: kind() must not be reached")
        }
        async fn account_info(&self) -> drive_storage::Result<AccountInfo> {
            panic!("stub provider: account_info() must not be reached")
        }
        async fn quota(&self) -> drive_storage::Result<Quota> {
            panic!("stub provider: quota() must not be reached")
        }
        async fn ensure_app_folder(&self, name: &str) -> drive_storage::Result<String> {
            Ok(format!("stub-{name}"))
        }
        async fn create_folder(
            &self,
            _parent_id: &str,
            _name: &str,
        ) -> drive_storage::Result<String> {
            panic!("stub provider: create_folder() must not be reached")
        }
        async fn list_folder(&self, _folder_id: &str) -> drive_storage::Result<FolderListing> {
            panic!("stub provider: list_folder() must not be reached")
        }
        async fn changes(&self, _page_token: Option<&str>) -> drive_storage::Result<ChangePage> {
            panic!("stub provider: changes() must not be reached")
        }
        async fn upload(
            &self,
            req: UploadRequest,
            progress: Option<&UploadProgress>,
        ) -> drive_storage::Result<RemoteFile> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            if call == 0 {
                let hook = self.on_first_upload.lock().unwrap().take();
                if let Some(hook) = hook {
                    hook();
                }
            }
            // Two ticks, so a test can tell a monotonic byte count from a single final one.
            if let Some(progress) = progress {
                progress(req.size_bytes / 2);
                progress(req.size_bytes);
            }
            if call < self.fail_first || self.refuse.as_deref() == Some(req.name.as_str()) {
                return Err(ProviderError::Transport("stub refuses this upload".into()));
            }
            Ok(RemoteFile {
                id: format!("drive-{}", req.name),
                name: req.name,
                mime_type: req.mime_type,
                size_bytes: Some(req.size_bytes),
                md5_checksum: None,
                parent_id: Some(req.parent_id),
                trashed: false,
                modified_at: None,
                can_download: true,
            })
        }
        async fn download(
            &self,
            _file_id: &str,
            _range: Option<Range>,
        ) -> drive_storage::Result<Vec<u8>> {
            panic!("stub provider: download() must not be reached")
        }
        async fn set_trashed(&self, _file_id: &str, _trashed: bool) -> drive_storage::Result<()> {
            panic!("stub provider: set_trashed() must not be reached")
        }
        async fn delete(&self, _file_id: &str) -> drive_storage::Result<()> {
            panic!("stub provider: delete() must not be reached")
        }
    }

    /// Temp directory removed on drop, since a failed assertion skips a trailing cleanup.
    struct TempTree(PathBuf);

    impl TempTree {
        fn new(label: &str) -> Self {
            static COUNTER: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "9drive-queue-{}-{label}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("create temp tree");
            Self(path)
        }

        /// Writes one file at the top of the tree and returns its absolute path.
        fn write(&self, name: &str) -> PathBuf {
            let path = self.0.join(name);
            std::fs::write(&path, PAYLOAD).expect("write temp file");
            path
        }
    }

    impl Drop for TempTree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn empty_db() -> SharedConn {
        shared(open_in_memory().expect("in-memory database"))
    }

    fn quota(account_id: i64, free: i64) -> AccountQuota {
        AccountQuota {
            account_id,
            limit_bytes: 100 * GB,
            usage_bytes: 100 * GB - free,
            usage_in_drive: Some(100 * GB - free),
            usage_in_drive_trash: Some(0),
            synced_at: 0,
        }
    }

    /// An initialised account with `free` bytes free. `router::select` skips anything else.
    fn seed(conn: &SharedConn, email: &str, free: i64) -> i64 {
        with_conn(conn, |c| {
            let account = accounts::create(c, "google", email, email)?;
            c.execute(
                "UPDATE accounts SET drive_folder_id = ?2 WHERE id = ?1",
                params![account.id, format!("folder-{}", account.id)],
            )?;
            accounts::upsert_quota(c, &quota(account.id, free))?;
            Ok(account.id)
        })
        .expect("seed an initialised account")
    }

    /// Takes every byte the account had, so the next routing sees no room.
    fn fill(conn: &SharedConn, account_id: i64) {
        with_conn(conn, |c| accounts::upsert_quota(c, &quota(account_id, 0)))
            .expect("fill the account");
    }

    /// Real files on disk, so the tasks carry the sizes `import::expand` measured.
    fn tasks(tree: &TempTree, names: &[&str]) -> Vec<UploadTask> {
        let paths: Vec<PathBuf> = names.iter().map(|name| tree.write(name)).collect();
        tasks_for(import::expand(&paths).expect("expand"), None).expect("tasks")
    }

    fn queue(policy: Policy, concurrency: usize) -> UploadQueue {
        UploadQueue::new(Router::new(policy), QueueConfig { concurrency, max_attempts: 3 })
    }

    fn rows(conn: &SharedConn) -> Vec<crate::models::FileItem> {
        with_conn(conn, |c| files::list(c, &FileFilter::default())).expect("list the mirror")
    }

    fn collector() -> (Arc<Mutex<Vec<ProgressEvent>>>, Arc<ProgressSink>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&seen);
        (seen, Arc::new(Box::new(move |event| sink.lock().unwrap().push(event))))
    }

    /// `(task_id, state, sent, total)` per upload event, dropping any other kind.
    fn upload_events(
        seen: &Arc<Mutex<Vec<ProgressEvent>>>,
    ) -> Vec<(i64, UploadState, u64, Option<u64>)> {
        seen.lock()
            .unwrap()
            .iter()
            .filter_map(|event| match event {
                ProgressEvent::Upload { task_id, state, sent, total, .. } => {
                    Some((*task_id, *state, *sent, *total))
                }
                _ => None,
            })
            .collect()
    }

    #[tokio::test]
    async fn every_task_finishes_and_is_visible_without_waiting_for_a_sync() {
        let conn = empty_db();
        let account = seed(&conn, "a@example.com", 100 * GB);
        let tree = TempTree::new("finish");
        let queue = queue(Policy::MostAvailable, 3);

        let report = queue
            .enqueue(
                &conn,
                Arc::new(Stub::default()),
                tasks(&tree, &["a.txt", "b.pdf", "c.bin"]),
                None,
            )
            .await
            .expect("enqueue");

        assert!(report.failed.is_empty(), "nothing failed: {:?}", report.failed);
        let mut task_ids: Vec<i64> = report.finished.iter().map(|(task_id, _)| *task_id).collect();
        task_ids.sort_unstable();
        assert_eq!(task_ids, [1, 2, 3]);

        let listed = rows(&conn);
        let mut row_ids: Vec<i64> = report.finished.iter().map(|(_, row_id)| *row_id).collect();
        row_ids.sort_unstable();
        let mut listed_ids: Vec<i64> = listed.iter().map(|file| file.id).collect();
        listed_ids.sort_unstable();
        assert_eq!(row_ids, listed_ids, "every success must name the row it wrote");
        assert!(listed.iter().all(|file| file.id > 0 && file.account_id == account));
    }

    #[tokio::test]
    async fn an_upload_lands_in_the_chosen_virtual_folder() {
        let conn = empty_db();
        seed(&conn, "a@example.com", 100 * GB);
        let tree = TempTree::new("folder");
        let folder_id =
            with_conn(&conn, |c| folders::create(c, "Reports", None)).expect("create folder").id;
        let queue = queue(Policy::MostAvailable, 1);
        let mut batch = tasks(&tree, &["a.txt"]);
        batch[0].folder_id = Some(folder_id);

        let report =
            queue.enqueue(&conn, Arc::new(Stub::default()), batch, None).await.expect("enqueue");

        assert!(report.failed.is_empty(), "nothing failed: {:?}", report.failed);
        let listed = rows(&conn);
        assert_eq!(listed.len(), 1, "one mirrored row");
        assert_eq!(
            listed[0].folder_id,
            Some(folder_id),
            "the mirror writes NULL, so the queue re-applies it"
        );
    }

    #[tokio::test]
    async fn a_failed_attempt_is_retried_and_mirrors_one_row() {
        let conn = empty_db();
        seed(&conn, "a@example.com", 100 * GB);
        let tree = TempTree::new("retry");
        let stub = Stub { fail_first: 2, ..Default::default() };
        let calls = Arc::clone(&stub.calls);
        let queue = queue(Policy::MostAvailable, 1);

        let report = queue
            .enqueue(&conn, Arc::new(stub), tasks(&tree, &["retry.txt"]), None)
            .await
            .expect("enqueue");

        assert_eq!(calls.load(Ordering::SeqCst), 3, "two failures plus the attempt that worked");
        assert_eq!(report.finished.len(), 1);
        assert!(report.failed.is_empty());
        let listed = rows(&conn);
        assert_eq!(listed.len(), 1, "the retry must not duplicate the row");
        assert_eq!(listed[0].name, "retry.txt");
        assert_eq!(listed[0].mime_type.as_deref(), Some("text/plain"));
    }

    #[tokio::test]
    async fn a_file_that_never_succeeds_is_failed_with_the_providers_code() {
        let conn = empty_db();
        seed(&conn, "a@example.com", 100 * GB);
        let tree = TempTree::new("doomed");
        let stub = Stub { refuse: Some("doomed.txt".into()), ..Default::default() };
        let queue = queue(Policy::MostAvailable, 1);

        let report = queue
            .enqueue(&conn, Arc::new(stub), tasks(&tree, &["doomed.txt", "fine.txt"]), None)
            .await
            .expect("one bad file is not a batch error");

        assert_eq!(report.failed, [(1, "transport".to_string())]);
        assert_eq!(report.finished.len(), 1);
        assert_eq!(report.finished[0].0, 2, "the file after the bad one still finishes");
        assert_eq!(rows(&conn).len(), 1);
    }

    #[tokio::test]
    async fn cancel_mid_run_cancels_the_rest_without_failing_them() {
        let conn = empty_db();
        seed(&conn, "a@example.com", 100 * GB);
        let tree = TempTree::new("cancel");
        let queue = Arc::new(queue(Policy::MostAvailable, 1));
        let handle = Arc::clone(&queue);
        let stub = Stub {
            on_first_upload: Mutex::new(Some(Box::new(move || handle.cancel()))),
            ..Default::default()
        };
        let (seen, sink) = collector();

        let report = queue
            .enqueue(
                &conn,
                Arc::new(stub),
                tasks(&tree, &["first.txt", "second.txt", "third.txt"]),
                Some(sink),
            )
            .await
            .expect("a cancel is not an error");

        assert_eq!(report.finished.len(), 1, "the file already uploading runs to completion");
        assert_eq!(report.finished[0].0, 1);
        assert!(report.failed.is_empty(), "a cancelled file is not a failure");
        let states: Vec<(i64, UploadState)> =
            upload_events(&seen).into_iter().map(|(id, state, _, _)| (id, state)).collect();
        assert!(states.contains(&(2, UploadState::Cancelled)), "{states:?}");
        assert!(states.contains(&(3, UploadState::Cancelled)), "{states:?}");
        assert_eq!(rows(&conn).len(), 1);
    }

    #[tokio::test]
    async fn pause_holds_the_queue_until_it_is_resumed() {
        let conn = empty_db();
        seed(&conn, "a@example.com", 100 * GB);
        let tree = TempTree::new("pause");
        let queue = Arc::new(queue(Policy::MostAvailable, 1));
        let handle = Arc::clone(&queue);
        let paused_by_stub = Arc::clone(&queue);
        let stub = Stub {
            on_first_upload: Mutex::new(Some(Box::new(move || paused_by_stub.pause()))),
            ..Default::default()
        };
        let (seen, sink) = collector();
        let queued = tasks(&tree, &["first.txt", "second.txt"]);
        let conn_for_runner = Arc::clone(&conn);
        let runner = tokio::spawn(async move {
            queue.enqueue(&conn_for_runner, Arc::new(stub), queued, Some(sink)).await
        });

        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!runner.is_finished(), "a paused queue must not dispatch the next file");
        handle.resume();
        let report = runner.await.expect("join the queue").expect("enqueue");

        assert_eq!(report.finished.len(), 2);
        let states: Vec<UploadState> =
            upload_events(&seen).into_iter().map(|(_, state, _, _)| state).collect();
        assert!(states.contains(&UploadState::Paused), "{states:?}");
    }

    #[tokio::test]
    async fn round_robin_spreads_a_batch_over_the_accounts() {
        let conn = empty_db();
        let first = seed(&conn, "a@example.com", 100 * GB);
        let second = seed(&conn, "b@example.com", 100 * GB);
        let tree = TempTree::new("spread");
        let queue = queue(Policy::RoundRobin, 1);

        let report = queue
            .enqueue(&conn, Arc::new(Stub::default()), tasks(&tree, &["one.txt", "two.txt"]), None)
            .await
            .expect("enqueue");

        assert_eq!(report.finished.len(), 2);
        let owners: Vec<i64> = rows(&conn).iter().map(|file| file.account_id).collect();
        assert!(owners.contains(&first) && owners.contains(&second), "got {owners:?}");
    }

    #[tokio::test]
    async fn a_retry_reroutes_away_from_an_account_that_has_filled_up() {
        let conn = empty_db();
        let first = seed(&conn, "a@example.com", 100 * GB);
        let second = seed(&conn, "b@example.com", 100 * GB);
        let tree = TempTree::new("reroute");
        let hook_conn = Arc::clone(&conn);
        let stub = Stub {
            fail_first: 1,
            on_first_upload: Mutex::new(Some(Box::new(move || fill(&hook_conn, first)))),
            ..Default::default()
        };
        let queue = queue(Policy::MostAvailable, 1);

        let report = queue
            .enqueue(&conn, Arc::new(stub), tasks(&tree, &["reroute.txt"]), None)
            .await
            .expect("enqueue");

        assert_eq!(report.finished.len(), 1);
        let owner = rows(&conn)[0].account_id;
        assert_eq!(owner, second, "the retry must not reuse the account that just filled");
    }

    #[tokio::test]
    async fn progress_never_goes_backwards_and_ends_finished() {
        let conn = empty_db();
        seed(&conn, "a@example.com", 100 * GB);
        let tree = TempTree::new("progress");
        // The first upload is refused after its ticks, so the retry restarts the byte count.
        let stub = Stub { fail_first: 1, ..Default::default() };
        let queue = queue(Policy::MostAvailable, 2);
        let (seen, sink) = collector();

        queue
            .enqueue(&conn, Arc::new(stub), tasks(&tree, &["one.txt", "two.txt"]), Some(sink))
            .await
            .expect("enqueue");

        let mut by_task: BTreeMap<i64, Vec<(UploadState, u64)>> = BTreeMap::new();
        for (task_id, state, sent, total) in upload_events(&seen) {
            assert_eq!(total, Some(PAYLOAD.len() as u64));
            by_task.entry(task_id).or_default().push((state, sent));
        }
        assert_eq!(by_task.len(), 2);
        for (task_id, ticks) in &by_task {
            let sent: Vec<u64> = ticks.iter().map(|(_, sent)| *sent).collect();
            assert!(
                sent.windows(2).all(|pair| pair[0] <= pair[1]),
                "sent must not fall back on a retry: task {task_id} sent {sent:?}"
            );
            assert_eq!(ticks.first().map(|(state, _)| *state), Some(UploadState::Started));
            assert_eq!(ticks.last().map(|(state, _)| *state), Some(UploadState::Finished));
        }
    }

    #[tokio::test]
    async fn a_successful_upload_stamps_the_accounts_last_used_at() {
        let conn = empty_db();
        let account = seed(&conn, "a@example.com", 100 * GB);
        let tree = TempTree::new("stamp");
        let queue = queue(Policy::MostAvailable, 1);
        assert_eq!(with_conn(&conn, |c| accounts::get(c, account)).unwrap().last_used_at, None);

        queue
            .enqueue(&conn, Arc::new(Stub::default()), tasks(&tree, &["stamped.txt"]), None)
            .await
            .expect("enqueue");

        assert!(with_conn(&conn, |c| accounts::get(c, account)).unwrap().last_used_at.is_some());
    }

    #[tokio::test]
    async fn tasks_the_queue_numbers_itself_are_numbered_in_order() {
        let conn = empty_db();
        seed(&conn, "a@example.com", 100 * GB);
        let tree = TempTree::new("unnumbered");
        let mut unnumbered = tasks(&tree, &["one.txt", "two.txt"]);
        for task in &mut unnumbered {
            task.task_id = 0;
        }
        let queue = queue(Policy::MostAvailable, 1);

        let report = queue
            .enqueue(&conn, Arc::new(Stub::default()), unnumbered, None)
            .await
            .expect("enqueue");

        let mut task_ids: Vec<i64> = report.finished.iter().map(|(task_id, _)| *task_id).collect();
        task_ids.sort_unstable();
        assert_eq!(task_ids, [1, 2]);
    }

    #[tokio::test]
    async fn an_empty_batch_is_not_an_error() {
        let conn = empty_db();
        let queue = queue(Policy::MostAvailable, 3);

        let report = queue
            .enqueue(&conn, Arc::new(Stub::default()), Vec::new(), None)
            .await
            .expect("enqueue");

        assert_eq!(report, UploadReport::default());
    }

    #[test]
    fn mime_types_cover_the_previewed_formats_and_fall_back_to_octet_stream() {
        let mime = |name: &str| mime_for(Path::new(name));
        assert_eq!(mime("report.pdf"), "application/pdf");
        assert_eq!(mime("Photo.JPEG"), "image/jpeg");
        assert_eq!(mime("photo.png"), "image/png");
        assert_eq!(mime("notes.txt"), "text/plain");
        assert_eq!(mime("clip.mp4"), "video/mp4");
        assert_eq!(mime("song.mp3"), "audio/mpeg");
        assert_eq!(mime("archive.zip"), "application/zip");
        assert_eq!(mime("data.json"), "application/json");
        assert_eq!(mime("thing.unknown"), "application/octet-stream");
        assert_eq!(mime("noextension"), "application/octet-stream");
    }

    #[test]
    fn a_batch_is_numbered_in_expand_order_and_named_after_the_file() {
        let tree = TempTree::new("batch");
        let candidates =
            import::expand(&[tree.write("b.pdf"), tree.write("a.txt")]).expect("expand");

        let built = tasks_for(candidates, None).expect("tasks");

        let names: Vec<&str> = built.iter().map(|task| task.name.as_str()).collect();
        assert_eq!(names, ["a.txt", "b.pdf"], "expand's order is the queue's order");
        assert_eq!(built.iter().map(|task| task.task_id).collect::<Vec<_>>(), [1, 2]);
        assert_eq!(built[1].mime_type, "application/pdf");
        assert_eq!(built[1].size_bytes, PAYLOAD.len() as u64);
    }

    #[test]
    fn a_bare_conversion_leaves_the_id_to_the_queue() {
        let candidate = ImportCandidate {
            source_path: PathBuf::from("/tmp/picked.txt"),
            relative_dir: None,
            size_bytes: 3,
        };

        let task: UploadTask = candidate.into();

        assert_eq!(task.task_id, 0, "only the queue hands out ids");
        assert_eq!(task.name, "picked.txt");
        assert_eq!(task.mime_type, "text/plain");
        assert_eq!(task.size_bytes, 3);
    }
}
