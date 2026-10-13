//! Google OAuth for 9Drive Desktop.
//!
//! Pure Rust on purpose: loopback listener, PKCE, code → token exchange, refresh and
//! the keychain that holds the refresh token. No `tauri` dependency, so `src-tauri`
//! calls this thin once F5 creates it. `authorize` is the whole connect flow; storing
//! and clearing refresh tokens stays with the caller so it can decide the account key.

pub mod client_id;
pub mod endpoints;
pub mod error;
pub mod flow;
pub mod google;
pub mod keychain;
pub mod listener;
pub mod pkce;
pub mod refresh;
pub mod token;

#[cfg(test)]
mod stub;

pub use client_id::{ClientId, resolve_client_id};
pub use endpoints::{DRIVE_SCOPE, Endpoints};
pub use error::{OAuthError, Result};
pub use flow::{Authorization, authorize};
pub use google::{AccountInfo, fetch_account_info, revoke};
pub use listener::Loopback;
pub use pkce::{CodeVerifier, State};
pub use refresh::refresh_access_token;
pub use token::{TokenSet, exchange_code};
