//! Storage provider abstraction for 9Drive Desktop.
//!
//! Every remote storage backend implements [`StorageProvider`]. Nothing outside this
//! crate is allowed to call a provider API directly.

pub mod error;
pub mod provider;
pub mod types;

pub use error::{ProviderError, Result};
pub use provider::StorageProvider;
pub use types::*;

/// Crate version, surfaced to the app so the UI can display it.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
