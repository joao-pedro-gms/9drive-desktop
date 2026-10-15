//! Uploads: expand what the user picked, route it, hand it to `drive-core`'s queue, and turn
//! its progress into the two events the frontend listens on.
//!
//! The queue already exists (`upload_queue.rs`) and owns concurrency, pause/resume, cancel
//! between files, retry per file and the progress sink. Nothing here re-implements it; the
//! only things this module adds are the ids the frontend needs, a way to address a running
//! batch, and the events.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicI64, Ordering};

use drive_core::error::AppError;
use drive_core::import;
use drive_core::routing::{Policy, Router};
use drive_core::upload_queue::{QueueConfig, UploadQueue, UploadTask};
use serde::Serialize;
use tauri::{AppHandle, State};

use crate::state::AppState;

use super::{IpcResult, fail, ok, upload_sink};

/// `UploadTask` does not derive `Serialize`, and these are the six fields `UploadTask` in
/// `ipc.ts` declares. `source_path` is a string because a path is not valid JSON on its own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UploadTaskOut {
    pub task_id: i64,
    pub source_path: String,
    pub name: String,
    pub mime_type: String,
    pub size_bytes: u64,
    pub folder_id: Option<i64>,
}

impl From<&UploadTask> for UploadTaskOut {
    fn from(task: &UploadTask) -> Self {
        Self {
            task_id: task.task_id,
            source_path: task.source_path.to_string_lossy().into_owned(),
            name: task.name.clone(),
            mime_type: task.mime_type.clone(),
            size_bytes: task.size_bytes,
            folder_id: task.folder_id,
        }
    }
}

/// One queue per batch, kept so a cancel can find the batch a task belongs to. The queue's own
/// flags live for the length of an `enqueue`, so sharing one queue across batches would let one
/// batch's cancel stop another's dispatch.
type Batch = Arc<UploadQueue>;

/// `task_id` -> the queue running it. An id is dropped when its batch ends, so a cancel that
/// arrives late reports `not_found` instead of stopping the next batch.
static BATCHES: LazyLock<tokio::sync::Mutex<HashMap<i64, Batch>>> =
    LazyLock::new(|| tokio::sync::Mutex::new(HashMap::new()));

/// `task_id` -> the task, kept **only while it can still be retried**: a failed upload, whose
/// file is on disk and whose row the queue did not write. Finished and cancelled tasks are
/// dropped when their batch ends, so this cannot grow without bound.
static RETRYABLE: LazyLock<tokio::sync::Mutex<HashMap<i64, UploadTask>>> =
    LazyLock::new(|| tokio::sync::Mutex::new(HashMap::new()));

/// Ids are unique for the life of the process, not per batch: the frontend keys its upload
/// list by `task_id`, so a second batch numbered from 1 would overwrite the first one's rows.
static NEXT_TASK_ID: AtomicI64 = AtomicI64::new(1);

fn next_task_id() -> i64 {
    NEXT_TASK_ID.fetch_add(1, Ordering::SeqCst)
}

/// Expands the picked paths (files or whole directories), numbers the batch and starts it.
///
/// Answers with the tasks, not with the result: the files upload in the background and report
/// through `upload://progress`.
#[tauri::command(rename_all = "snake_case")]
pub async fn start_upload_batch(
    app: AppHandle,
    state: State<'_, AppState>,
    paths: Vec<String>,
    folder_id: Option<i64>,
) -> IpcResult<Vec<UploadTaskOut>> {
    let picked: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
    // A missing path, an unreadable directory: rejected here, before anything is queued.
    let candidates = ok(import::expand(&picked))?;

    let first = next_task_id();
    let tasks: Vec<UploadTask> = candidates
        .into_iter()
        .enumerate()
        .map(|(index, candidate)| UploadTask {
            task_id: first + index as i64,
            folder_id,
            ..candidate.into()
        })
        .collect();
    let out: Vec<UploadTaskOut> = tasks.iter().map(UploadTaskOut::from).collect();

    if !tasks.is_empty() {
        spawn(app, &state, tasks).await.map_err(fail)?;
    }
    Ok(out)
}

/// Stops dispatching the batch this task belongs to.
///
/// The queue cancels *between files*: the rest of the batch is marked cancelled and nothing
/// new starts, but a file already inside `provider.upload` runs to completion, because that is
/// one await the queue has no token to interrupt (documented in `upload_queue.rs`). There is no
/// per-file cancel in the queue, so cancelling one file cancels the remainder of its batch --
/// and every one of those files says so on `upload://progress`.
#[tauri::command(rename_all = "snake_case")]
pub async fn cancel_upload(task_id: i64) -> IpcResult<()> {
    let queue = { BATCHES.lock().await.get(&task_id).cloned() };
    let Some(queue) = queue else {
        return Err(fail(AppError::not_found("upload task", task_id)));
    };
    queue.cancel();
    Ok(())
}

/// Runs one task again, in a batch of its own, so the retry is re-routed from scratch.
///
/// Only a failed task is retryable: the others have either finished (the mirror row exists) or
/// were cancelled (the file was never sent), and keeping their paths alive would let a retry
/// upload a file the user has already seen the outcome of. The entry is left in place when the
/// new batch cannot even be started, so a second attempt is still possible.
#[tauri::command(rename_all = "snake_case")]
pub async fn retry_upload(
    app: AppHandle,
    state: State<'_, AppState>,
    task_id: i64,
) -> IpcResult<()> {
    let task = { RETRYABLE.lock().await.get(&task_id).cloned() }
        .ok_or_else(|| fail(AppError::not_found("upload task", task_id)))?;
    spawn(app, &state, vec![task]).await.map_err(fail)?;
    Ok(())
}

/// Picks the account for the batch, builds its queue and runs it on its own task.
///
/// The queue is pinned to one account on purpose. `UploadQueue::enqueue` is handed a single
/// provider for the whole batch while it re-routes every task on its own, so a free router
/// would upload a file with one account's credentials and mirror it under another. Pinning
/// keeps the local row and the account that really holds the bytes the same; the cost is that
/// one batch never spans accounts.
async fn spawn<R: tauri::Runtime>(
    app: AppHandle<R>,
    state: &AppState,
    tasks: Vec<UploadTask>,
) -> Result<(), AppError> {
    let biggest = tasks.iter().map(|task| task.size_bytes).max().unwrap_or(0);
    let choice = Router::new(state.upload_policy()).select(&state.conn(), biggest)?;
    let provider = state.provider(choice.account.id).await?;
    let queue: Batch = Arc::new(UploadQueue::new(
        Router::new(Policy::Fixed(vec![choice.account.id])),
        // `QueueConfig::default()` is 3 at a time with 3 attempts each. Neither is a setting
        // yet; when it is, it belongs in the same place as `upload_policy`.
        QueueConfig::default(),
    ));

    let ids: Vec<i64> = tasks.iter().map(|task| task.task_id).collect();
    {
        let mut batches = BATCHES.lock().await;
        let mut retryable = RETRYABLE.lock().await;
        for task in &tasks {
            batches.insert(task.task_id, Arc::clone(&queue));
            retryable.insert(task.task_id, task.clone());
        }
    }

    let conn = state.conn();
    let sink = upload_sink(app);
    tauri::async_runtime::spawn(async move {
        let outcome = queue.enqueue(&conn, provider, tasks, Some(sink)).await;
        // Forget the batch first, so a cancel racing the end cannot reach the next one.
        {
            let mut batches = BATCHES.lock().await;
            let mut retryable = RETRYABLE.lock().await;
            for id in &ids {
                batches.remove(id);
            }
            match &outcome {
                Ok(report) => {
                    let failed: Vec<i64> =
                        report.failed.iter().map(|(task_id, _)| *task_id).collect();
                    retryable.retain(|task_id, _| failed.contains(task_id));
                }
                Err(err) => tracing::warn!(error = %err, "an upload batch stopped early"),
            }
        }
        if let Ok(report) = &outcome {
            for (task_id, code) in &report.failed {
                tracing::warn!(task_id, code, "upload failed");
            }
        }
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_ids_are_unique_across_batches() {
        let first = next_task_id();
        let second = next_task_id();
        assert!(first > 0, "0 means the task was never routed");
        assert_eq!(second, first + 1);
    }

    #[test]
    fn the_wire_shape_keeps_every_field_the_frontend_holds() {
        let task = UploadTask {
            task_id: 7,
            source_path: PathBuf::from("/tmp/report.pdf"),
            name: "report.pdf".into(),
            mime_type: "application/pdf".into(),
            size_bytes: 2048,
            folder_id: Some(3),
        };
        let out = UploadTaskOut::from(&task);

        assert_eq!(out.task_id, 7);
        assert_eq!(out.source_path, "/tmp/report.pdf");
        assert_eq!(out.name, "report.pdf");
        assert_eq!(out.mime_type, "application/pdf");
        assert_eq!(out.size_bytes, 2048);
        assert_eq!(out.folder_id, Some(3));
    }

    #[test]
    fn a_task_with_no_virtual_folder_says_so() {
        let task = UploadTask {
            task_id: 1,
            source_path: PathBuf::from("C:/pictures/a.png"),
            name: "a.png".into(),
            mime_type: "image/png".into(),
            size_bytes: 10,
            folder_id: None,
        };
        assert_eq!(UploadTaskOut::from(&task).folder_id, None);
        // The path reaches the provider as the string the task carries, separators and all.
        assert_eq!(UploadTaskOut::from(&task).source_path, "C:/pictures/a.png");
    }

    #[test]
    fn an_import_becomes_tasks_numbered_from_the_shared_counter() {
        let dir = std::env::temp_dir().join(format!("9drive-upload-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let picked = dir.join("notes.txt");
        std::fs::write(&picked, b"hello").expect("write");

        let candidates = import::expand(std::slice::from_ref(&picked)).expect("expand");
        let first = next_task_id();
        let tasks: Vec<UploadTask> = candidates
            .into_iter()
            .map(|candidate| UploadTask { task_id: first, folder_id: None, ..candidate.into() })
            .collect();

        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].name, "notes.txt");
        assert_eq!(tasks[0].mime_type, "text/plain");
        assert_eq!(tasks[0].size_bytes, 5);
        assert_eq!(tasks[0].task_id, first, "the counter owns the ids, not the batch");
    }

    #[test]
    fn a_missing_picked_path_is_rejected_before_anything_is_queued() {
        let ghost = std::env::temp_dir().join("9drive-does-not-exist-9drive");
        let err = import::expand(&[ghost]).expect_err("missing");
        assert_eq!(err.code(), "invalid_input");
    }
}
