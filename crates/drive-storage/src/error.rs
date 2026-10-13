//! Provider failures, with the stable codes a later phase serialises across IPC.

use thiserror::Error;

/// Result alias for every fallible provider call.
pub type Result<T> = std::result::Result<T, ProviderError>;

#[derive(Debug, Error)]
pub enum ProviderError {
    /// The caller passed something unusable; no request was ever made.
    #[error("invalid argument: {0}")]
    InvalidArgument(String),

    #[error("unauthorized: {0}")]
    Unauthorized(String),

    #[error("forbidden: {0}")]
    Forbidden(String),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("conflict: {0}")]
    Conflict(String),

    #[error("rate limited (retry after {retry_after:?}s): {message}")]
    RateLimited { message: String, retry_after: Option<u64> },

    #[error("transport failure: {0}")]
    Transport(String),

    #[error("could not decode provider response: {0}")]
    Decode(String),

    #[error("provider error: {0}")]
    Provider(String),

    #[error("local file error: {0}")]
    LocalIo(String),
}

impl ProviderError {
    /// Stable machine-readable code; callers switch on it, never on the message.
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidArgument(_) => "invalid_argument",
            Self::Unauthorized(_) => "unauthorized",
            Self::Forbidden(_) => "forbidden",
            Self::NotFound(_) => "not_found",
            Self::Conflict(_) => "conflict",
            Self::RateLimited { .. } => "rate_limited",
            Self::Transport(_) => "transport",
            Self::Decode(_) => "decode",
            Self::Provider(_) => "provider",
            Self::LocalIo(_) => "local_io",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_variants() -> Vec<ProviderError> {
        vec![
            ProviderError::InvalidArgument("range 10..5".into()),
            ProviderError::Unauthorized("token expired".into()),
            ProviderError::Forbidden("scope missing".into()),
            ProviderError::NotFound("file 42".into()),
            ProviderError::Conflict("name already taken".into()),
            ProviderError::RateLimited { message: "slow down".into(), retry_after: Some(30) },
            ProviderError::Transport("connection reset".into()),
            ProviderError::Decode("missing fields.kind".into()),
            ProviderError::Provider("quota exceeded".into()),
            ProviderError::LocalIo("read EACCES".into()),
        ]
    }

    #[test]
    fn code_is_stable_per_variant() {
        let codes: Vec<&str> = all_variants().iter().map(ProviderError::code).collect();
        assert_eq!(
            codes,
            [
                "invalid_argument",
                "unauthorized",
                "forbidden",
                "not_found",
                "conflict",
                "rate_limited",
                "transport",
                "decode",
                "provider",
                "local_io"
            ]
        );
    }

    #[test]
    fn rate_limited_carries_retry_after_seconds() {
        let err = ProviderError::RateLimited { message: "slow down".into(), retry_after: Some(30) };
        match err {
            ProviderError::RateLimited { retry_after, .. } => assert_eq!(retry_after, Some(30)),
            other => panic!("expected rate limited, got {other:?}"),
        }
        assert!(err.to_string().contains("30"));

        let no_hint = ProviderError::RateLimited { message: "slow down".into(), retry_after: None };
        assert!(matches!(no_hint, ProviderError::RateLimited { retry_after: None, .. }));
    }

    #[test]
    fn every_variant_is_a_std_error() {
        fn as_dyn(err: &ProviderError) -> &dyn std::error::Error {
            err
        }
        assert!(!as_dyn(&all_variants()[0]).to_string().is_empty());
    }
}
