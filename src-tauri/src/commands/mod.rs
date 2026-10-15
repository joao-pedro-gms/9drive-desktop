//! The IPC boundary: one command per wrapper in `src/lib/ipc.ts`, and nothing else.
//!
//! Every command here validates its input, calls `drive-core` (through `AppState` for anything
//! remote) and maps `AppError` to `IpcError`. No Drive HTTP, no SQL, no domain rules — the
//! frontend contract in `ipc.ts` is the only thing that decides a name or a shape.
//!
//! Every one of them carries `rename_all = "snake_case"`: Tauri looks an argument up under its
//! camelCase name by default, and `ipc.ts` sends `account_id`, `file_id`, `task_id` and the
//! rest exactly as the Rust field is spelled.

pub mod accounts;
pub mod files;
pub mod folders;
pub mod settings;
pub mod uploads;

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use drive_core::error::{AppError, IpcError, Result as CoreResult};
use drive_core::progress::{ProgressEvent, ProgressSink, UploadState};
use drive_storage::error::ProviderError;
use tauri::{AppHandle, Emitter, Runtime};

use crate::state::AppState;

/// What every command rejects with: `{ code, message }`, the shape `IpcError` in `ipc.ts`
/// switches on. `IpcError::from(&AppError)` already carries the stable code.
pub(crate) type IpcResult<T> = std::result::Result<T, IpcError>;

pub(crate) const UPLOAD_PROGRESS: &str = "upload://progress";
pub(crate) const UPLOAD_DONE: &str = "upload://done";
pub(crate) const SYNC_PROGRESS: &str = "sync://progress";
pub(crate) const ACCOUNTS_CHANGED: &str = "accounts://changed";

/// Unix epoch **seconds**, the unit every `drive-core` timestamp uses. Its `now_unix` is
/// crate-private, and this copy has to agree with it or `last_used_at` sorts centuries away.
pub(crate) fn now_unix() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// `?` for a domain call: one conversion for the whole boundary.
pub(crate) fn ok<T>(result: CoreResult<T>) -> IpcResult<T> {
    result.map_err(|err| IpcError::from(&err))
}

/// A domain error built at the call site rather than returned by one.
pub(crate) fn fail(err: AppError) -> IpcError {
    IpcError::from(&err)
}

/// Rejected input. `AppError` has no I/O arm, so a local filesystem or plugin failure is
/// reported as the user-fixable input error it usually is.
pub(crate) fn invalid(message: impl Into<String>) -> IpcError {
    fail(AppError::InvalidInput(message.into()))
}

/// A provider failure keeps its own code: `AppError::Storage` forwards `ProviderError::code()`.
pub(crate) fn storage(err: ProviderError) -> IpcError {
    fail(AppError::Storage(err))
}

/// `?` for a provider call that answers `ProviderError` directly rather than wrapping it.
pub(crate) fn ok_provider<T>(result: drive_storage::error::Result<T>) -> IpcResult<T> {
    result.map_err(storage)
}

/// The queue will report nothing more about a task in one of these states, which is exactly
/// when `upload://done` fires. `paused` is not terminal: the queue resumes from there.
pub(crate) fn is_terminal(state: UploadState) -> bool {
    matches!(state, UploadState::Finished | UploadState::Cancelled | UploadState::Failed)
}

/// The sink the upload queue reports into. `drive-core` describes what happened; this is where
/// each variant picks its channel (AGENTS.md: progress is events, never polling).
pub(crate) fn upload_sink<R: Runtime>(app: AppHandle<R>) -> Arc<ProgressSink> {
    Arc::new(Box::new(move |event: ProgressEvent| {
        let ProgressEvent::Upload { state, .. } = &event else { return };
        // A failed emit means the window is gone, and there is nobody left to tell.
        let _ = app.emit(UPLOAD_PROGRESS, &event);
        if is_terminal(*state) {
            let _ = app.emit(UPLOAD_DONE, &event);
        }
    }))
}

/// The same for `sync_google`, which the engine reports into.
pub(crate) fn sync_sink<R: Runtime>(app: AppHandle<R>) -> Arc<ProgressSink> {
    Arc::new(Box::new(move |event: ProgressEvent| {
        if let ProgressEvent::Sync { .. } = event {
            let _ = app.emit(SYNC_PROGRESS, &event);
        }
    }))
}

/// The full account list after a connect, a disconnect or a quota change. Read fresh so the
/// payload is never a stale copy.
pub(crate) fn emit_accounts<R: Runtime>(app: &AppHandle<R>, state: &AppState) {
    match drive_core::db::with_conn(&state.conn(), drive_core::repos::accounts::list) {
        Ok(list) => {
            let _ = app.emit(ACCOUNTS_CHANGED, list);
        }
        Err(err) => tracing::warn!(error = %err, "could not emit accounts://changed"),
    }
}

/// `OAuthError` has no `AppError` arm and the orphan rule bars a cross-crate `From`, so the
/// mapping is spelled out here exactly as `state::oauth_error` spells it: two errors need
/// handling a provider failure does not — "reconnect the account" and "nothing is configured".
pub(crate) fn oauth(err: drive_oauth::OAuthError) -> AppError {
    use drive_oauth::OAuthError;

    match err {
        OAuthError::Reauthenticate(message) => {
            AppError::Storage(ProviderError::Unauthorized(message))
        }
        OAuthError::Config(message) => AppError::Storage(ProviderError::InvalidArgument(message)),
        other => AppError::Storage(ProviderError::Provider(other.to_string())),
    }
}

/// A client id is required before anything OAuth can run; `AppState::client_id` answers an
/// empty string rather than an error, because Settings is where that gets reported.
pub(crate) fn client_id(state: &AppState) -> Result<String, IpcError> {
    let id = state.client_id();
    if id.is_empty() {
        return Err(invalid("no Google OAuth client id is configured: set one in Settings"));
    }
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every wrapper the frontend calls must have a command behind it. Read from the two
    /// sources of truth rather than a hand-copied list, so a rename in `ipc.ts` fails here.
    const IPC_TS: &str = include_str!("../../../src/lib/ipc.ts");
    const LIB_RS: &str = include_str!("../lib.rs");
    const SOURCES: [(&str, &str); 5] = [
        ("accounts.rs", include_str!("accounts.rs")),
        ("files.rs", include_str!("files.rs")),
        ("folders.rs", include_str!("folders.rs")),
        ("settings.rs", include_str!("settings.rs")),
        ("uploads.rs", include_str!("uploads.rs")),
    ];

    /// The `invokeCommand('name')` argument of each wrapper, in source order.
    fn called_commands() -> Vec<&'static str> {
        let mut names = Vec::new();
        let mut rest = IPC_TS;
        while let Some(at) = rest.find("invokeCommand('") {
            rest = &rest[at + "invokeCommand('".len()..];
            let end = rest.find('\'').expect("a closing quote on every wrapper");
            names.push(&rest[..end]);
            rest = &rest[end..];
        }
        names
    }

    fn registered_commands() -> Vec<&'static str> {
        let mut names = Vec::new();
        let mut rest = LIB_RS;
        while let Some(at) = rest.find("commands::") {
            rest = &rest[at + "commands::".len()..];
            let end = rest.find(',').expect("a handler list ends every entry with a comma");
            let path = &rest[..end];
            names.push(path.rsplit("::").next().expect("a module path"));
            rest = &rest[end..];
        }
        names
    }

    #[test]
    fn every_command_the_frontend_calls_is_registered() {
        let registered = registered_commands();
        assert!(registered.len() > 30, "the handler list looks truncated: {registered:?}");
        for name in called_commands() {
            assert!(registered.contains(&name), "ipc.ts calls `{name}`, which is not registered");
        }
    }

    #[test]
    fn a_command_is_registered_once() {
        let registered = registered_commands();
        let mut sorted = registered.clone();
        sorted.sort_unstable();
        let count = sorted.len();
        sorted.dedup();
        assert_eq!(sorted.len(), count, "a command registered twice: {registered:?}");
    }

    /// The trap this guards: Tauri reads an argument under its camelCase name unless the
    /// command says otherwise, and `ipc.ts` sends snake_case throughout.
    #[test]
    fn every_command_takes_its_arguments_in_snake_case() {
        for (name, source) in SOURCES {
            assert_eq!(
                source.matches("#[tauri::command]").count(),
                0,
                "{name} has a command that would read its arguments as camelCase"
            );
            assert!(
                source.contains(r#"#[tauri::command(rename_all = "snake_case")]"#),
                "{name} lost the snake_case annotation"
            );
        }
    }

    #[test]
    fn terminal_states_are_the_ones_the_queue_stops_reporting() {
        assert!(is_terminal(UploadState::Finished));
        assert!(is_terminal(UploadState::Cancelled));
        assert!(is_terminal(UploadState::Failed));
        assert!(!is_terminal(UploadState::Started));
        assert!(!is_terminal(UploadState::Running));
        assert!(!is_terminal(UploadState::Paused), "a paused task is resumed, not finished");
    }

    #[test]
    fn an_app_error_reaches_the_frontend_with_the_code_it_switches_on() {
        assert_eq!(fail(AppError::not_found("file", 7)).code, "not_found");
        assert_eq!(invalid("blank").code, "invalid_input");
        assert_eq!(
            storage(ProviderError::Unauthorized("invalid_grant".into())).code,
            "unauthorized"
        );
        assert_eq!(
            ok(Err::<(), _>(AppError::InvalidInput("x".into()))).unwrap_err().code,
            "invalid_input"
        );
    }

    #[test]
    fn oauth_failures_keep_the_codes_the_connect_flow_switches_on() {
        use drive_oauth::OAuthError;

        assert_eq!(
            fail(oauth(OAuthError::Reauthenticate("invalid_grant".into()))).code,
            "unauthorized"
        );
        assert_eq!(fail(oauth(OAuthError::Config("no client id".into()))).code, "invalid_argument");
        assert_eq!(fail(oauth(OAuthError::Cancelled("denied".into()))).code, "provider");
    }

    #[test]
    fn the_local_clock_agrees_with_the_unit_drive_core_stores() {
        let now = now_unix();
        assert!(now > 1_600_000_000, "seconds, not milliseconds: {now}");
        assert!(now < 100_000_000_000);
    }
}
