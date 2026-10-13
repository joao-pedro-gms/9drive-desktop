//! Domain types, SQLite access, sync engine and upload routing for 9Drive Desktop.
//!
//! This crate owns everything that does not need Tauri and does not talk to a remote
//! API. It must not depend on `tauri`: Tauri commands stay thin and call into here.

pub mod db;
pub mod error;
pub mod import;
pub mod models;
pub mod progress;
pub mod repos;
pub mod routing;
pub mod sync_engine;
pub mod upload_queue;

pub use error::{AppError, IpcError, Result};
pub use progress::{ProgressEvent, ProgressSink, SyncState, UploadState};

/// Crate version, surfaced to the app so the UI can display it.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
