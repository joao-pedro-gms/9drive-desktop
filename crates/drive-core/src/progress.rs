//! Progress reporting for long-running work.
//!
//! Tauri does not exist until F5, so nothing here emits an event. The rule is that the
//! orchestration layer describes *what happened* and F5 adapts each variant to its channel:
//! [`ProgressEvent::Upload`] becomes `upload://progress`, [`ProgressEvent::Sync`] becomes
//! `sync://progress` (AGENTS.md: progress is events, never polling).

use serde::{Deserialize, Serialize};

/// One progress tick. Serializable because it crosses the IPC boundary unchanged once the Tauri
/// layer attaches its own channel name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProgressEvent {
    /// A queued file moved forward. `task_id` is the upload queue's own id, not a `files.id`:
    /// local rows are a mirror of Drive, so there is no row to name until the upload succeeds.
    Upload {
        task_id: i64,
        name: String,
        /// Known from the moment routing picks a target, so the UI can show which account is
        /// taking the file before the first byte lands.
        account_id: i64,
        /// Cumulative bytes accepted by the provider so far.
        sent: u64,
        total: Option<u64>,
        /// Terminal states. `Failed` carries the code the UI shows, not a provider message.
        state: UploadState,
    },
    /// A sync run for one account. `processed` counts change entries applied, not files,
    /// because one entry can be a rename, a move or a delete.
    Sync { account_id: i64, processed: u64, state: SyncState },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UploadState {
    Started,
    Running,
    /// Stored by the provider and mirrored locally.
    Finished,
    /// Paused by the user. `sent` stays where it was; the queue resumes from there.
    Paused,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncState {
    Started,
    Running,
    /// The run reached the last page and its `newStartPageToken` was stored.
    Finished,
    /// The stored page token is now invalid (Drive expires them after 30 days). The next run
    /// starts from scratch; the mirror is rebuilt by the same code path.
    Restarted,
    Failed,
}

/// Where progress goes. `None` is a normal case, not a fallback: the smoke test and the CLI
/// paths have nobody listening, and threading a no-op through them would be noise.
pub type ProgressSink = Box<dyn Fn(ProgressEvent) + Send + Sync>;

/// Reports progress to `sink`, if there is one. Every orchestration function takes
/// `Option<&ProgressSink>` rather than a sink so callers cannot forget to handle absence.
pub fn report(sink: Option<&ProgressSink>, event: ProgressEvent) {
    if let Some(sink) = sink {
        sink(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    fn collector() -> (Arc<Mutex<Vec<ProgressEvent>>>, ProgressSink) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink_seen = Arc::clone(&seen);
        (seen, Box::new(move |event| sink_seen.lock().unwrap().push(event)))
    }

    fn upload_event(state: UploadState) -> ProgressEvent {
        ProgressEvent::Upload {
            task_id: 7,
            name: "report.pdf".into(),
            account_id: 1,
            sent: 512,
            total: Some(1024),
            state,
        }
    }

    #[test]
    fn absent_sink_is_not_an_error() {
        // Nothing to assert beyond "this must not panic": the smoke path has no listener.
        report(None, upload_event(UploadState::Started));
    }

    #[test]
    fn events_reach_the_sink_in_order() {
        let (seen, sink) = collector();
        report(Some(&sink), upload_event(UploadState::Started));
        report(Some(&sink), upload_event(UploadState::Finished));

        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 2);
        assert!(matches!(
            seen[0],
            ProgressEvent::Upload { state: UploadState::Started, sent: 512, .. }
        ));
    }

    #[test]
    fn the_frontend_can_tell_the_variants_apart_without_parsing_a_message() {
        // F5 attaches the channel name; this asserts the payload is already unambiguous.
        let upload = serde_json::to_value(upload_event(UploadState::Failed)).unwrap();
        assert_eq!(upload["kind"], "upload");
        assert_eq!(upload["state"], "failed");
        assert_eq!(upload["sent"], 512);

        let sync = serde_json::to_value(ProgressEvent::Sync {
            account_id: 3,
            processed: 42,
            state: SyncState::Restarted,
        })
        .unwrap();
        assert_eq!(sync["kind"], "sync");
        assert_eq!(sync["state"], "restarted");
        assert_eq!(sync["account_id"], 3);
    }
}
