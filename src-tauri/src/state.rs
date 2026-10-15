//! The state every Tauri command reads: one shared SQLite connection plus the provider cache.
//!
//! Nothing here reaches a Google API directly — a command asks for a [`StorageProvider`] and
//! talks to it through the trait (`AGENTS.md`).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use drive_core::db::{self, SharedConn};
use drive_core::error::{AppError, Result};
use drive_core::repos::{accounts, settings};
use drive_core::routing::Policy;
use drive_storage::{GoogleDrive, StorageProvider, error::ProviderError};
use rusqlite::Connection;
use tokio::sync::Mutex;

/// Settings key holding the client id pasted in Settings; overrides the compiled default.
const CLIENT_ID_KEY: &str = "oauth.client_id";
/// Settings key holding the upload routing policy.
const UPLOAD_POLICY_KEY: &str = "upload.policy";

/// What `client_id` returns when neither the override nor the compiled default resolves.
/// `provider` turns it back into an error, so the empty string never reaches Drive.
const NO_CLIENT_ID: &str = "";
/// How long before the access token actually expires the cache stops being trusted, so a
/// request is never sent with a token that expires in flight.
const REFRESH_MARGIN: Duration = Duration::from_secs(60);
/// `expires_in` is optional in Google's response. Assuming an hour keeps a missing value from
/// turning into a provider that is rebuilt on every command.
const DEFAULT_TOKEN_SECONDS: u64 = 3600;

/// True while the access token is far enough from expiring to keep using it, so a request is
/// never sent with a token that runs out in flight.
fn token_is_fresh(expires_at: Instant) -> bool {
    expires_at.saturating_duration_since(Instant::now()) > REFRESH_MARGIN
}

/// The lifetime to cache a provider for. `expires_in` is optional in Google's response;
/// assuming an hour keeps a missing value from rebuilding the provider on every command.
fn expires_at(expires_in: Option<u64>) -> Instant {
    Instant::now() + Duration::from_secs(expires_in.unwrap_or(DEFAULT_TOKEN_SECONDS))
}

/// The provider cached per account and the moment its access token stops being usable.
type Cached = (Arc<dyn StorageProvider>, Instant);

pub struct AppState {
    conn: SharedConn,
    /// `tokio::sync::Mutex` because the guard is held across the token refresh `.await`;
    /// a `std::sync` guard could not be, and a `rusqlite` `&Connection` never is.
    providers: Mutex<HashMap<i64, Cached>>,
}

impl AppState {
    /// Applies pending migrations, so an existing database and a fresh one behave the same.
    pub fn new(conn: Connection) -> Result<Self> {
        db::migrate(&conn)?;
        Ok(Self { conn: db::shared(conn), providers: Mutex::new(HashMap::new()) })
    }

    pub fn conn(&self) -> SharedConn {
        Arc::clone(&self.conn)
    }

    /// The Settings override when there is one, otherwise the compiled default. Empty when
    /// neither resolves: Settings is where that gets reported, not an error per command.
    pub fn client_id(&self) -> String {
        let stored = self.setting(CLIENT_ID_KEY).ok().flatten();
        drive_oauth::client_id::resolve_client_id(stored.as_deref())
            .map(|id| id.as_str().to_string())
            .unwrap_or_else(|_| NO_CLIENT_ID.to_string())
    }

    /// An unreadable or unrecognised policy falls back to the crate default instead of
    /// failing an upload over a setting the user cannot see from the error.
    pub fn upload_policy(&self) -> Policy {
        let stored = self.setting(UPLOAD_POLICY_KEY).ok().flatten();
        stored.and_then(|value| value.parse().ok()).unwrap_or_default()
    }

    pub fn has_accounts(&self) -> Result<bool> {
        db::with_conn(&self.conn, |c| Ok(!accounts::list(c)?.is_empty()))
    }

    /// The account's provider, built on first use and then cached until its access token is
    /// close to expiring: Drive needs a short-lived access token, so rebuilding one per command
    /// would re-exchange the refresh token every time and hammer Google's token endpoint, while
    /// caching it forever would start failing an hour in (issue #26).
    pub async fn provider(&self, account_id: i64) -> Result<Arc<dyn StorageProvider>> {
        let mut cache = self.providers.lock().await;
        if let Some(entry) = cache.get(&account_id).filter(|entry| token_is_fresh(entry.1)) {
            return Ok(Arc::clone(&entry.0));
        }

        let email = db::with_conn(&self.conn, |c| Ok(accounts::get(c, account_id)?.email))?;
        let client_id = self.client_id();
        if client_id == NO_CLIENT_ID {
            return Err(AppError::InvalidInput(
                "no Google OAuth client id is configured: set one in Settings".into(),
            ));
        }
        // The keychain holds the refresh token only (AGENTS.md); the access token it is traded
        // for stays in memory here and is never written anywhere.
        let refresh_token =
            drive_oauth::keychain::read_refresh_token(&email).map_err(oauth_error)?;
        let tokens = drive_oauth::refresh_access_token(
            &reqwest::Client::new(),
            &drive_oauth::endpoints::Endpoints::default(),
            &client_id,
            &refresh_token,
        )
        .await
        .map_err(oauth_error)?;

        let provider: Arc<dyn StorageProvider> = Arc::new(GoogleDrive::new(tokens.access_token));
        cache.insert(account_id, (Arc::clone(&provider), expires_at(tokens.expires_in)));
        Ok(provider)
    }

    fn setting(&self, key: &str) -> Result<Option<String>> {
        db::with_conn(&self.conn, |c| settings::get(c, key))
    }
}

/// Keeps the OAuth code the frontend switches on. `AppError` has no OAuth arm and the orphan
/// rule bars a `From` impl across crates, so the mapping lives here: the two errors that need
/// different handling from the user are "reconnect the account" and "nothing is configured",
/// and everything else is a provider failure.
fn oauth_error(err: drive_oauth::OAuthError) -> AppError {
    use drive_oauth::OAuthError;

    let error = match err {
        OAuthError::Reauthenticate(message) => ProviderError::Unauthorized(message),
        OAuthError::Config(message) => ProviderError::InvalidArgument(message),
        other => ProviderError::Provider(other.to_string()),
    };
    AppError::Storage(error)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> AppState {
        AppState::new(db::open_in_memory().expect("in-memory database")).expect("state")
    }

    fn set(state: &AppState, key: &str, value: &str) {
        let conn = state.conn();
        db::with_conn(&conn, |c| settings::set(c, key, value)).expect("setting written");
    }

    #[test]
    fn new_applies_migrations_to_an_empty_database() {
        let conn = state().conn();
        let applied: i64 = db::with_conn(&conn, |c| {
            Ok(c.query_row("SELECT COUNT(*) FROM _migrations", [], |row| row.get(0))?)
        })
        .unwrap();
        assert!(applied > 0, "an unmigrated database would break every command");
    }

    #[test]
    fn conn_hands_out_the_same_shared_handle() {
        let state = state();
        assert!(Arc::ptr_eq(&state.conn(), &state.conn()));
    }

    #[test]
    fn the_settings_override_wins_over_the_compiled_client_id() {
        let state = state();
        set(&state, CLIENT_ID_KEY, "pasted-id.apps.googleusercontent.com");
        assert_eq!(state.client_id(), "pasted-id.apps.googleusercontent.com");
    }

    #[test]
    fn a_blank_client_id_is_not_an_override() {
        let state = state();
        set(&state, CLIENT_ID_KEY, "   ");
        // Must match having no setting at all, not turn into a blank id.
        let blank = state.client_id();
        db::with_conn(&state.conn(), |c| settings::delete(c, CLIENT_ID_KEY)).unwrap();
        assert_eq!(blank, state.client_id());
    }

    #[test]
    fn upload_policy_parses_the_setting_and_falls_back_to_the_default() {
        let state = state();
        assert_eq!(state.upload_policy(), Policy::default());

        set(&state, UPLOAD_POLICY_KEY, "round_robin");
        assert_eq!(state.upload_policy(), Policy::RoundRobin);

        set(&state, UPLOAD_POLICY_KEY, "fixed:3,1");
        assert_eq!(state.upload_policy(), Policy::Fixed(vec![3, 1]));

        for junk in ["nonsense", "fixed:", "fixed:a"] {
            set(&state, UPLOAD_POLICY_KEY, junk);
            assert_eq!(state.upload_policy(), Policy::default(), "{junk}");
        }
    }

    #[test]
    fn has_accounts_follows_the_accounts_table() {
        let state = state();
        assert!(!state.has_accounts().unwrap());

        let conn = state.conn();
        db::with_conn(&conn, |c| {
            accounts::create(c, "google", "Test User", "a@example.com")?;
            Ok(())
        })
        .unwrap();
        assert!(state.has_accounts().unwrap());

        db::with_conn(&conn, |c| {
            let account = accounts::list(c)?.remove(0);
            accounts::delete(c, account.id)
        })
        .unwrap();
        assert!(!state.has_accounts().unwrap());
    }

    /// `dyn StorageProvider` is not `Debug`, so the error is taken by hand instead of
    /// `unwrap_err`.
    fn provider_error(state: &AppState, account_id: i64) -> AppError {
        let Err(err) = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("runtime")
            .block_on(state.provider(account_id))
        else {
            panic!("account {account_id} has no reachable provider")
        };
        err
    }

    #[test]
    fn provider_for_an_unknown_account_is_not_found() {
        let state = state();
        // No network and no keychain entry may be reached before the account lookup.
        assert_eq!(provider_error(&state, 404).code(), "not_found");
    }

    #[test]
    fn an_unconfigured_client_id_is_reported_before_the_keychain_is_touched() {
        let state = state();
        let conn = state.conn();
        db::with_conn(&conn, |c| {
            accounts::create(c, "google", "Test User", "no-client@example.com")?;
            Ok(())
        })
        .unwrap();
        // Written outside `with_conn`: the shared lock is not reentrant.
        set(&state, CLIENT_ID_KEY, "");
        assert_eq!(state.client_id(), NO_CLIENT_ID);

        let err = provider_error(&state, 1);
        assert_eq!(err.code(), "invalid_input");
        assert!(err.to_string().contains("Settings"), "{err}");
    }

    #[test]
    fn a_cached_provider_is_dropped_before_its_token_expires() {
        // Issue #26: caching forever meant a long-running app started failing after an hour.
        let hour_out = expires_at(Some(3600));
        assert!(token_is_fresh(hour_out), "a full-hour token must be reused");

        assert!(token_is_fresh(expires_at(None)), "a missing expires_in assumes an hour");

        let half_a_minute_left = Instant::now() + Duration::from_secs(30);
        assert!(!token_is_fresh(half_a_minute_left), "30s left is inside the margin");

        let already_gone = Instant::now() - Duration::from_secs(1);
        assert!(!token_is_fresh(already_gone), "an expired token must never be reused");
    }

    #[test]
    fn oauth_errors_keep_the_code_the_frontend_switches_on() {
        use drive_oauth::OAuthError;

        assert_eq!(
            oauth_error(OAuthError::Reauthenticate("invalid_grant".into())).code(),
            "unauthorized"
        );
        assert_eq!(
            oauth_error(OAuthError::Config("no client id".into())).code(),
            "invalid_argument"
        );

        let cancelled = oauth_error(OAuthError::Cancelled("closed the window".into()));
        assert_eq!(cancelled.code(), "provider");
        assert!(cancelled.to_string().contains("closed the window"));
    }
}
