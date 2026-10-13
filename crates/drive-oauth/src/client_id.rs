//! Resolution of the OAuth client id: Settings override first, compiled default second.

use std::fmt;

use crate::error::{OAuthError, Result};

/// Empty when the build had no `GOOGLE_CLIENT_ID`, so a contributor without a Cloud
/// project still compiles and is asked for one in Settings instead.
const COMPILED_DEFAULT: Option<&str> = match option_env!("GOOGLE_CLIENT_ID") {
    Some(id) if !id.is_empty() => Some(id),
    _ => None,
};

/// A resolved client id, plus where it came from so Settings can show a hint.
pub struct ClientId {
    value: String,
    compiled_default: bool,
}

impl ClientId {
    pub fn as_str(&self) -> &str {
        &self.value
    }

    /// True when the compiled-in default was used, which Google may eventually expire.
    pub fn used_compiled_default(&self) -> bool {
        self.compiled_default
    }
}

// A client id is a credential-like value that must not reach a log line.
impl fmt::Debug for ClientId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClientId")
            .field("value", &"<redacted>")
            .field("compiled_default", &self.compiled_default)
            .finish()
    }
}

/// `override_id` wins when it is not blank; otherwise fall back to the compiled
/// default and finally fail so Settings can ask for one.
pub fn resolve_client_id(override_id: Option<&str>) -> Result<ClientId> {
    if let Some(id) = override_id.map(str::trim).filter(|id| !id.is_empty()) {
        return Ok(ClientId { value: id.to_string(), compiled_default: false });
    }

    match COMPILED_DEFAULT {
        Some(id) => Ok(ClientId { value: id.to_string(), compiled_default: true }),
        None => {
            Err(OAuthError::Config("set a Google OAuth Desktop app client id in Settings".into()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn override_wins_over_the_compiled_default() {
        let resolved = resolve_client_id(Some("  pasted-id.apps.googleusercontent.com  "))
            .expect("override resolves");
        assert_eq!(resolved.as_str(), "pasted-id.apps.googleusercontent.com");
        assert!(!resolved.used_compiled_default());
    }

    #[test]
    fn a_blank_override_is_not_an_override() {
        for blank in ["", "   "] {
            // Must behave exactly like passing no override: the compiled default
            // comes back, or Settings is asked for an id.
            let with_blank = resolve_client_id(Some(blank));
            let without = resolve_client_id(None);
            assert_eq!(with_blank.is_ok(), without.is_ok(), "{blank:?}");
            assert_eq!(
                with_blank.map(|id| id.as_str().to_string()).ok(),
                without.map(|id| id.as_str().to_string()).ok(),
                "{blank:?}"
            );
        }
    }

    /// Exercises whichever branch this build has: a client id compiled in at test
    /// time, or the "ask in Settings" error.
    #[test]
    fn the_compiled_default_matches_this_build() {
        match COMPILED_DEFAULT {
            Some(id) => {
                let resolved = resolve_client_id(None).expect("compiled default resolves");
                assert_eq!(resolved.as_str(), id);
                assert!(resolved.used_compiled_default());
            }
            None => {
                let err = resolve_client_id(None).expect_err("no id must fail");
                assert_eq!(err.code(), "config");
                assert!(err.to_string().contains("Settings"));
            }
        }
    }

    #[test]
    fn debug_does_not_leak_the_client_id() {
        let resolved = resolve_client_id(Some("secret-id.apps.googleusercontent.com")).expect("id");
        assert!(!format!("{resolved:?}").contains("secret-id"));
    }
}
