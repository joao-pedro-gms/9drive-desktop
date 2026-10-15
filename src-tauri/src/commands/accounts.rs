//! Accounts and the app itself: the Google connect flow, the account list, quota, sync, and the
//! two housekeeping commands Settings uses.
//!
//! The connect flow is split in two because `ipc.ts` calls it that way: `begin_google_auth`
//! binds the loopback listener, stores the PKCE verifier and opens the browser itself, and
//! `complete_google_auth` waits for the redirect that browser is already carrying.

use std::sync::OnceLock;

use drive_core::db::{self, SharedConn};
use drive_core::error::AppError;
use drive_core::models::{Account, AccountQuota};
use drive_core::progress::ProgressEvent;
use drive_core::repos::accounts;
use drive_core::sync_engine::{self, SyncOutcome};
use drive_oauth::Loopback;
use drive_oauth::endpoints::Endpoints;
use drive_oauth::google::AccountInfo;
use drive_oauth::keychain;
use drive_oauth::pkce::{CodeVerifier, State as OAuthState};
use drive_oauth::token::{self, TokenSet};
use serde::Serialize;
use tauri::{AppHandle, Manager, State};

use crate::state::AppState;

use super::{IpcResult, client_id, emit_accounts, fail, invalid, oauth, ok, storage, sync_sink};

/// What `begin_google_auth` hands back so the frontend can show, or open, the consent screen.
#[derive(Debug, Serialize)]
pub struct AuthRequest {
    pub auth_url: String,
}

/// What `app_info` reports at boot.
#[derive(Debug, Serialize)]
pub struct AppInfo {
    pub version: String,
    pub has_accounts: bool,
}

/// `sync_engine::SyncOutcome` does not derive `Serialize`, and the frontend expects these four
/// numbers per account.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct SyncOutcomeOut {
    pub processed: u64,
    pub upserted: u64,
    pub deleted: u64,
    /// False when the run restarted from scratch because its page token had expired.
    pub incremental: bool,
}

impl From<SyncOutcome> for SyncOutcomeOut {
    fn from(outcome: SyncOutcome) -> Self {
        Self {
            processed: outcome.processed,
            upserted: outcome.upserted,
            deleted: outcome.deleted,
            incremental: outcome.incremental,
        }
    }
}

/// A consent screen waiting for its redirect. Held between the two commands, so `begin` owns
/// the socket and the verifier and `complete` spends them.
struct PendingAuth {
    loopback: Loopback,
    redirect_uri: String,
    verifier: CodeVerifier,
    state: OAuthState,
}

/// One connect at a time: a second `begin` while one is pending would orphan the first
/// listener, and its port would never receive the redirect.
static PENDING: tokio::sync::Mutex<Option<PendingAuth>> = tokio::sync::Mutex::const_new(None);

/// One `reqwest` client for the whole app, so repeated Drive calls reuse the connection
/// instead of paying a TLS handshake each time.
fn http() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(reqwest::Client::new)
}

fn endpoints() -> Endpoints {
    Endpoints::default()
}

#[tauri::command(rename_all = "snake_case")]
pub fn list_accounts(state: State<'_, AppState>) -> IpcResult<Vec<Account>> {
    ok(db::with_conn(&state.conn(), accounts::list))
}

#[tauri::command(rename_all = "snake_case")]
pub fn app_info(state: State<'_, AppState>) -> IpcResult<AppInfo> {
    Ok(AppInfo {
        // The crate version, which the workspace keeps equal to `tauri.conf.json`.
        version: env!("CARGO_PKG_VERSION").to_string(),
        has_accounts: ok(state.has_accounts())?,
    })
}

/// Opens the app's data folder in the system file manager. The directory is created first:
/// on a first run it may not exist yet and the shell would report nothing useful.
#[tauri::command(rename_all = "snake_case")]
pub fn open_app_dir(app: AppHandle) -> IpcResult<()> {
    use tauri_plugin_opener::OpenerExt;

    let dir = app
        .path()
        .app_data_dir()
        .map_err(|err| invalid(format!("no app data directory on this platform: {err}")))?;
    if !dir.exists() {
        std::fs::create_dir_all(&dir)
            .map_err(|err| invalid(format!("could not create {}: {err}", dir.display())))?;
    }
    app.opener()
        .open_path(dir.to_string_lossy().into_owned(), None::<&str>)
        .map_err(|err| invalid(format!("could not open {}: {err}", dir.display())))
}

/// Starts the consent screen: binds the loopback listener, keeps the PKCE verifier and state,
/// and opens the browser itself (the frontend is not expected to).
#[tauri::command(rename_all = "snake_case")]
pub async fn begin_google_auth(
    app: AppHandle,
    state: State<'_, AppState>,
) -> IpcResult<AuthRequest> {
    use tauri_plugin_opener::OpenerExt;

    // Checked before anything is opened, so a second call does not leave a consent tab behind
    // for a flow that is then refused.
    if PENDING.lock().await.is_some() {
        return Err(fail(AppError::Conflict("a Google sign-in is already in progress".into())));
    }
    let id = client_id(&state)?;
    let loopback = Loopback::bind().await.map_err(oauth).map_err(fail)?;
    let redirect_uri = loopback.redirect_uri();
    let verifier = CodeVerifier::generate().map_err(oauth).map_err(fail)?;
    let pkce_state = OAuthState::generate().map_err(oauth).map_err(fail)?;
    let auth_url = token::authorize_url(
        &endpoints(),
        &id,
        &redirect_uri,
        drive_oauth::DRIVE_SCOPE,
        &verifier,
        &pkce_state,
    );

    // A browser that will not open leaves a listener nothing can reach, so the failure ends
    // the flow instead of parking a pending auth nobody will complete.
    if let Err(err) = app.opener().open_url(auth_url.clone(), None::<&str>) {
        return Err(invalid(format!("could not open the browser: {err}")));
    }

    *PENDING.lock().await =
        Some(PendingAuth { loopback, redirect_uri, verifier, state: pkce_state });
    Ok(AuthRequest { auth_url })
}

/// Finishes the consent screen: takes the code off the loopback listener, exchanges it, stores
/// the refresh token **in the OS keychain only** (issue #24), upserts the account, and reports
/// the new account list.
#[tauri::command(rename_all = "snake_case")]
pub async fn complete_google_auth(
    app: AppHandle,
    state: State<'_, AppState>,
) -> IpcResult<Account> {
    let id = client_id(&state)?;
    let pending = PENDING
        .lock()
        .await
        .take()
        .ok_or_else(|| fail(AppError::Conflict("no Google sign-in is in progress".into())))?;

    let code = pending.loopback.wait_for_code(&pending.state).await.map_err(oauth).map_err(fail)?;
    let tokens = token::exchange_code(
        http(),
        &endpoints(),
        &id,
        &pending.redirect_uri,
        &code,
        &pending.verifier,
    )
    .await
    .map_err(oauth)
    .map_err(fail)?;
    let profile = fetch_profile(&tokens).await?;

    store_grant(&profile, &tokens)?;
    let account = upsert_account(&state.conn(), &profile)?;

    // Best effort from here: the grant exists and the row is written, so failing the command
    // would tell the user "not connected" while their account is. The `9drive` folder and the
    // quota bar are repaired by the next sync (`sync_engine` runs `ensure_app_folder` every
    // time) and by the Quota page's refresh.
    if let Err(err) = prime(&state, account.id).await {
        tracing::warn!(account_id = account.id, error = %err.message, "connected, but folder/quota not recorded yet");
    }

    emit_accounts(&app, &state);
    Ok(account)
}

/// Reads the account profile with the access token the exchange just returned.
async fn fetch_profile(tokens: &TokenSet) -> IpcResult<AccountInfo> {
    drive_oauth::fetch_account_info(http(), &endpoints(), &tokens.access_token)
        .await
        .map_err(oauth)
        .map_err(fail)
}

/// The refresh token goes to the keychain and nowhere else — never SQLite, never a config file
/// (AGENTS.md). It is never logged either, so this function is the only place that sees it.
fn store_grant(profile: &AccountInfo, tokens: &TokenSet) -> IpcResult<()> {
    match tokens.refresh_token.as_deref() {
        Some(refresh) => {
            keychain::store_refresh_token(&profile.email, refresh).map_err(oauth).map_err(fail)
        }
        None => {
            // Google omits it outside `prompt=consent`, which the authorize URL sets. An
            // account with no stored token simply cannot refresh, and the next connect fixes
            // it; failing here would discard a perfectly good access.
            tracing::warn!("Google returned no refresh token for {}", profile.email);
            Ok(())
        }
    }
}

/// Inserts the account, or updates the row a previous connect of the same email created:
/// `accounts.email` has no UNIQUE index, so a plain insert would leave two rows for one Drive.
/// Stamping `last_used_at` is what makes an account "recently used".
fn upsert_account(conn: &SharedConn, profile: &AccountInfo) -> IpcResult<Account> {
    let name = profile.name.as_deref().unwrap_or(&profile.email);
    ok(db::with_conn(conn, |conn| {
        let existing = accounts::list(conn)?
            .into_iter()
            .find(|account| account.email == profile.email)
            .map(|account| account.id);
        let id = match existing {
            Some(id) => id,
            None => accounts::create(conn, "google", name, &profile.email)?.id,
        };
        // Either way the account is being used right now, which is what the stamp records.
        accounts::update(conn, id, name, Some(super::now_unix()))?;
        accounts::get(conn, id)
    }))
}

/// Records the account's `9drive` folder and its quota. Both go through `AppState::provider`,
/// the only door to a provider, which refreshes and caches the access token.
async fn prime(state: &AppState, account_id: i64) -> IpcResult<()> {
    let provider = ok(state.provider(account_id).await)?;
    let folder_id = provider.ensure_app_folder("9drive").await.map_err(storage)?;
    ok(db::with_conn(&state.conn(), |conn| {
        accounts::set_drive_folder_id(conn, account_id, &folder_id)
    }))?;
    let _ = sync_quota_with(state, account_id).await?;
    Ok(())
}

/// Revokes the grant, then clears the keychain entry and the row whatever the revoke did
/// (issue #28). A user who asked to disconnect must not be left with a live grant they
/// cannot see, nor with a row whose refresh token is gone.
#[tauri::command(rename_all = "snake_case")]
pub async fn disconnect_account(
    app: AppHandle,
    state: State<'_, AppState>,
    account_id: i64,
) -> IpcResult<()> {
    let account = ok(db::with_conn(&state.conn(), |conn| accounts::get(conn, account_id)))?;

    match keychain::read_refresh_token(&account.email) {
        Ok(refresh) => {
            if let Err(err) = drive_oauth::revoke(http(), &endpoints(), &refresh).await {
                // Never the refresh token itself: only Google's complaint about it.
                tracing::warn!(account_id, error = %err, "revoke failed; disconnecting anyway");
            }
        }
        Err(err) => tracing::warn!(account_id, error = %err, "no stored grant to revoke"),
    }
    if let Err(err) = keychain::delete_refresh_token(&account.email) {
        tracing::warn!(account_id, error = %err, "could not clear the stored grant");
    }

    // Cascades to quota, sync state and files.
    ok(db::with_conn(&state.conn(), |conn| accounts::delete(conn, account_id)))?;
    emit_accounts(&app, &state);
    Ok(())
}

/// Reads the account's quota from Drive and stores it. Storage is what
/// `routing::Router::select` reads, so a stale quota is a misrouted upload.
#[tauri::command(rename_all = "snake_case")]
pub async fn sync_account_quota(
    state: State<'_, AppState>,
    account_id: i64,
) -> IpcResult<AccountQuota> {
    sync_quota_with(&state, account_id).await
}

/// The same command under the name `docs/rewrite-plan.md` §7 and the settings screen use.
/// `ipc.ts` only calls `sync_account_quota`; both go through one body.
#[tauri::command(rename_all = "snake_case")]
pub async fn refresh_quota(state: State<'_, AppState>, account_id: i64) -> IpcResult<AccountQuota> {
    sync_account_quota(state, account_id).await
}

async fn sync_quota_with(state: &AppState, account_id: i64) -> IpcResult<AccountQuota> {
    // Answers `not_found` for an unknown account before any network call.
    ok(db::with_conn(&state.conn(), |conn| accounts::get(conn, account_id)))?;
    let provider = ok(state.provider(account_id).await)?;
    let quota = provider.quota().await.map_err(storage)?;
    let as_i64 = |bytes: u64| i64::try_from(bytes).unwrap_or(i64::MAX);
    let stored = AccountQuota {
        account_id,
        limit_bytes: as_i64(quota.limit_bytes),
        usage_bytes: as_i64(quota.usage_bytes),
        usage_in_drive: quota.usage_in_drive.map(as_i64),
        usage_in_drive_trash: quota.usage_in_drive_trash.map(as_i64),
        synced_at: super::now_unix(),
    };
    ok(db::with_conn(&state.conn(), |conn| {
        accounts::upsert_quota(conn, &stored)?;
        accounts::get_quota(conn, account_id)
    }))
}

/// Runs the change feed for every connected account, one at a time, and reports what each did.
#[tauri::command(rename_all = "snake_case")]
pub async fn sync_google(
    app: AppHandle,
    state: State<'_, AppState>,
) -> IpcResult<Vec<SyncOutcomeOut>> {
    let sink = sync_sink(app);
    let account_ids = ok(db::with_conn(&state.conn(), |conn| {
        Ok(accounts::list(conn)?.into_iter().map(|account| account.id).collect::<Vec<_>>())
    }))?;

    let mut outcomes = Vec::with_capacity(account_ids.len());
    for account_id in account_ids {
        let provider = match state.provider(account_id).await {
            Ok(provider) => provider,
            Err(err) => {
                // One revoked grant must not stop the accounts that still work.
                report_failed(&sink, account_id, &err);
                continue;
            }
        };
        match sync_engine::sync_account(&state.conn(), provider.as_ref(), account_id, Some(&sink))
            .await
        {
            Ok(outcome) => outcomes.push(outcome.into()),
            Err(err) => report_failed(&sink, account_id, &err),
        }
    }
    Ok(outcomes)
}

/// The engine reports its own terminal states; this is the one it does not, so the frontend's
/// `syncing[account_id]` entry is cleared instead of spinning forever.
fn report_failed(
    sink: &std::sync::Arc<drive_core::progress::ProgressSink>,
    account_id: i64,
    err: &AppError,
) {
    tracing::warn!(account_id, error = %err, "sync failed");
    drive_core::progress::report(
        Some(sink.as_ref()),
        ProgressEvent::Sync { account_id, processed: 0, state: drive_core::SyncState::Failed },
    );
}

#[cfg(test)]
mod tests {
    use drive_oauth::client_id::resolve_client_id;

    use super::*;

    fn conn() -> SharedConn {
        db::shared(db::open_in_memory().expect("in-memory database"))
    }

    fn profile(email: &str, name: Option<&str>) -> AccountInfo {
        AccountInfo { name: name.map(str::to_string), email: email.to_string() }
    }

    #[test]
    fn a_first_connect_writes_one_row_and_stamps_it_as_used() {
        let conn = conn();
        let account =
            upsert_account(&conn, &profile("a@example.com", Some("Test User"))).expect("connect");

        assert_eq!(account.email, "a@example.com");
        assert_eq!(account.name, "Test User");
        assert_eq!(account.provider, "google");
        assert!(account.last_used_at.is_some(), "a connected account is being used");
        assert!(account.drive_folder_id.is_none(), "prime() writes it after this");
        assert_eq!(db::with_conn(&conn, accounts::list).expect("list").len(), 1);
    }

    #[test]
    fn reconnecting_the_same_email_reuses_its_row() {
        let conn = conn();
        let first = upsert_account(&conn, &profile("a@example.com", Some("Old Name")))
            .expect("first connect");
        let again = upsert_account(&conn, &profile("a@example.com", Some("New Name")))
            .expect("second connect");

        assert_eq!(again.id, first.id, "two rows for one Drive would double every listing");
        assert_eq!(again.name, "New Name");
        assert_eq!(again.created_at, first.created_at, "a reconnect is not a new account");
        assert_eq!(db::with_conn(&conn, accounts::list).expect("list").len(), 1);
    }

    #[test]
    fn an_account_without_a_display_name_falls_back_to_its_email() {
        let conn = conn();
        let account =
            upsert_account(&conn, &profile("nameless@example.com", None)).expect("connect");
        assert_eq!(account.name, "nameless@example.com");
    }

    #[test]
    fn a_second_account_is_a_second_row() {
        let conn = conn();
        upsert_account(&conn, &profile("a@example.com", Some("A"))).expect("first");
        upsert_account(&conn, &profile("b@example.com", Some("B"))).expect("second");

        let listed = db::with_conn(&conn, accounts::list).expect("list");
        assert_eq!(listed.len(), 2);
    }

    #[test]
    fn a_pasted_client_id_is_what_the_authorize_url_gets() {
        let resolved = resolve_client_id(Some("pasted.apps.googleusercontent.com")).expect("id");
        assert_eq!(resolved.as_str(), "pasted.apps.googleusercontent.com");
    }

    #[test]
    fn the_token_client_is_shared_rather_than_rebuilt_per_call() {
        assert!(std::ptr::eq(http(), http()), "one connection pool for the whole app");
    }

    #[test]
    fn a_sync_outcome_serializes_with_the_four_fields_the_frontend_reads() {
        // The engine's own struct is not `Serialize`; this is the one that crosses IPC, so
        // the mapping has to keep every field.
        let out = SyncOutcomeOut::from(SyncOutcome {
            processed: 7,
            upserted: 5,
            deleted: 2,
            incremental: false,
        });
        assert_eq!(out.processed, 7);
        assert_eq!(out.upserted, 5);
        assert_eq!(out.deleted, 2);
        assert!(!out.incremental);
    }
}
