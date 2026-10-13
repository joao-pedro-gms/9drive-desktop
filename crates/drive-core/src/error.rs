//! Crate error type plus the `{ code, message }` shape sent across the IPC boundary.

use serde::{Deserialize, Serialize};

/// Result alias for every fallible call in this crate.
pub type Result<T> = std::result::Result<T, AppError>;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),

    #[error("{entity} {id} not found")]
    NotFound { entity: &'static str, id: i64 },

    #[error("invalid input: {0}")]
    InvalidInput(String),

    #[error("conflict: {0}")]
    Conflict(String),

    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),
}

impl AppError {
    pub fn not_found(entity: &'static str, id: i64) -> Self {
        Self::NotFound { entity, id }
    }

    /// Stable machine-readable code; the frontend switches on it, never on `message`.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Database(_) => "database_error",
            Self::NotFound { .. } => "not_found",
            Self::InvalidInput(_) => "invalid_input",
            Self::Conflict(_) => "conflict",
            Self::Serde(_) => "serde_error",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IpcError {
    pub code: String,
    pub message: String,
}

impl From<&AppError> for IpcError {
    fn from(err: &AppError) -> Self {
        Self { code: err.code().to_string(), message: err.to_string() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_variants() -> Vec<AppError> {
        vec![
            AppError::Database(rusqlite::Error::InvalidQuery),
            AppError::not_found("folder", 7),
            AppError::InvalidInput("name must not be blank".into()),
            AppError::Conflict("folder 1 cannot be moved into its own subtree".into()),
            AppError::from(serde_json::from_str::<i32>("not json").unwrap_err()),
        ]
    }

    #[test]
    fn ipc_error_maps_each_variant_to_its_code() {
        let codes: Vec<&str> = all_variants().iter().map(AppError::code).collect();
        assert_eq!(
            codes,
            ["database_error", "not_found", "invalid_input", "conflict", "serde_error"]
        );

        let db = AppError::Database(rusqlite::Error::InvalidQuery);
        let ipc = IpcError::from(&db);
        assert_eq!(ipc.code, "database_error");
        assert_eq!(ipc.message, db.to_string());
    }

    #[test]
    fn ipc_error_serializes_as_code_and_message() {
        let ipc = IpcError::from(&AppError::not_found("file", 42));
        let json = serde_json::to_value(&ipc).unwrap();
        assert_eq!(json["code"], "not_found");
        assert_eq!(json["message"], "file 42 not found");
        assert_eq!(serde_json::from_value::<IpcError>(json).unwrap(), ipc);
    }

    #[test]
    fn rusqlite_error_converts_through_from() {
        let err: AppError = rusqlite::Error::InvalidQuery.into();
        assert!(matches!(err, AppError::Database(_)));
    }
}
