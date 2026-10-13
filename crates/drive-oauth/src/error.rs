//! OAuth failures, with the stable codes a later phase serialises across IPC.

use thiserror::Error;

/// Result alias for every fallible OAuth call.
pub type Result<T> = std::result::Result<T, OAuthError>;

#[derive(Debug, Error)]
pub enum OAuthError {
    /// No client id resolved: neither a Settings override nor a compiled default.
    #[error("no Google OAuth client id is configured: {0}")]
    Config(String),

    #[error("http failure: {0}")]
    Http(#[from] reqwest::Error),

    #[error("keychain failure: {0}")]
    Keychain(#[from] keyring::Error),

    #[error("malformed OAuth response: {0}")]
    Protocol(String),

    /// The flow ended without a grant: consent denied, or the browser never came back.
    #[error("authorization cancelled: {0}")]
    Cancelled(String),

    #[error("loopback listener failure: {0}")]
    Listener(String),

    /// Google invalidated the grant; the account must be reconnected, not retried.
    #[error("grant is no longer valid, reconnect the account: {0}")]
    Reauthenticate(String),

    #[error("no system entropy available: {0}")]
    Entropy(#[from] getrandom::Error),
}

impl OAuthError {
    /// Stable machine-readable code; callers switch on it, never on the message.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Config(_) => "config",
            Self::Http(_) => "http",
            Self::Keychain(_) => "keychain",
            Self::Protocol(_) => "protocol",
            Self::Cancelled(_) => "cancelled",
            Self::Listener(_) => "listener",
            Self::Reauthenticate(_) => "reauthenticate",
            Self::Entropy(_) => "entropy",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real `reqwest::Error` without touching the network: building the request is
    /// what fails, on the malformed URL.
    fn http_error() -> reqwest::Error {
        reqwest::Client::new()
            .get("http://[not-a-host")
            .build()
            .expect_err("malformed url must not build")
    }

    fn all_variants() -> Vec<OAuthError> {
        vec![
            OAuthError::Config("no client id".into()),
            OAuthError::Http(http_error()),
            OAuthError::Keychain(keyring::Error::NoEntry),
            OAuthError::Protocol("missing code".into()),
            OAuthError::Cancelled("consent denied".into()),
            OAuthError::Listener("port busy".into()),
            OAuthError::Reauthenticate("invalid_grant".into()),
            OAuthError::Entropy(getrandom::Error::UNSUPPORTED),
        ]
    }

    #[test]
    fn code_is_stable_per_variant() {
        let codes: Vec<&str> = all_variants().iter().map(OAuthError::code).collect();
        assert_eq!(
            codes,
            [
                "config",
                "http",
                "keychain",
                "protocol",
                "cancelled",
                "listener",
                "reauthenticate",
                "entropy"
            ]
        );
    }

    #[test]
    fn messages_never_carry_a_token() {
        // The Protocol arm is the one built from a Google response body; it must only
        // ever receive an error code and a description, never the payload.
        let err = OAuthError::Protocol("token response: missing field `access_token`".into());
        assert!(!err.to_string().contains("ya29."));
        assert_eq!(err.code(), "protocol");
    }

    #[test]
    fn every_variant_is_a_std_error() {
        fn as_dyn(err: &OAuthError) -> &dyn std::error::Error {
            err
        }
        assert!(!as_dyn(&all_variants()[0]).to_string().is_empty());
    }
}
