//! The Google endpoints, overridable so tests can point at a local stub.

/// `drive` is a restricted scope: changing it invalidates existing authorizations.
pub const DRIVE_SCOPE: &str = "https://www.googleapis.com/auth/drive";

/// Google endpoints, overridable so tests can point the flow at a local stub.
#[derive(Debug, Clone)]
pub struct Endpoints {
    pub authorize: String,
    pub token: String,
    pub revoke: String,
    pub drive: String,
}

// Deliberately not `#[derive(Default)]`: an empty endpoint would silently break the
// flow instead of falling back to Google.
impl Default for Endpoints {
    fn default() -> Self {
        Self {
            authorize: "https://accounts.google.com/o/oauth2/v2/auth".into(),
            token: "https://oauth2.googleapis.com/token".into(),
            revoke: "https://oauth2.googleapis.com/revoke".into(),
            drive: "https://www.googleapis.com/drive/v3".into(),
        }
    }
}
