//! The trait every remote storage backend implements.

use async_trait::async_trait;

use crate::error::Result;
use crate::types::{
    AccountInfo, ChangePage, FolderListing, ProviderKind, Quota, Range, RemoteFile, UploadRequest,
};

/// Reports cumulative bytes the provider has accepted so far. Called after every chunk the
/// provider confirms, so the caller can drive a progress bar without polling.
pub type UploadProgress = dyn Fn(u64) + Send + Sync;

/// Remote storage backend. Workspace documents are never downloaded: they are exported, the plan
/// caps an export at 10 MB, and the caller must check [`RemoteFile::can_download`] first.
#[async_trait]
pub trait StorageProvider: Send + Sync {
    fn kind(&self) -> ProviderKind;

    async fn account_info(&self) -> Result<AccountInfo>;

    async fn quota(&self) -> Result<Quota>;

    /// Idempotently returns the id of the app folder named `name`, creating it when absent.
    async fn ensure_app_folder(&self, name: &str) -> Result<String>;

    async fn create_folder(&self, parent_id: &str, name: &str) -> Result<String>;

    async fn list_folder(&self, folder_id: &str) -> Result<FolderListing>;

    /// Incremental change feed; the returned token is the next call's `page_token`.
    async fn changes(&self, page_token: Option<&str>) -> Result<ChangePage>;

    /// Streams [`UploadRequest::source_path`] to the provider as a resumable upload, reading it
    /// one chunk at a time. `progress`, when given, is told the cumulative bytes accepted so
    /// far after every chunk the provider confirms.
    async fn upload(
        &self,
        req: UploadRequest,
        progress: Option<&UploadProgress>,
    ) -> Result<RemoteFile>;

    /// shortcut: the whole body is buffered into a `Vec<u8>` because no stream dependency is in
    /// scope yet; upgrade to a byte stream when large media preview (F5) needs it.
    async fn download(&self, file_id: &str, range: Option<Range>) -> Result<Vec<u8>>;

    async fn set_trashed(&self, file_id: &str, trashed: bool) -> Result<()>;

    async fn delete(&self, file_id: &str) -> Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ProviderError;

    /// Only proves the trait is implementable and usable as a trait object; no async test
    /// runner is available in this crate yet, so no method is actually awaited.
    struct FakeProvider;

    #[async_trait]
    impl StorageProvider for FakeProvider {
        fn kind(&self) -> ProviderKind {
            ProviderKind::Google
        }
        async fn account_info(&self) -> Result<AccountInfo> {
            Err(fake())
        }
        async fn quota(&self) -> Result<Quota> {
            Err(fake())
        }
        async fn ensure_app_folder(&self, _name: &str) -> Result<String> {
            Err(fake())
        }
        async fn create_folder(&self, _parent_id: &str, _name: &str) -> Result<String> {
            Err(fake())
        }
        async fn list_folder(&self, _folder_id: &str) -> Result<FolderListing> {
            Err(fake())
        }
        async fn changes(&self, _page_token: Option<&str>) -> Result<ChangePage> {
            Err(fake())
        }
        async fn upload(
            &self,
            _req: UploadRequest,
            _progress: Option<&UploadProgress>,
        ) -> Result<RemoteFile> {
            Err(fake())
        }
        async fn download(&self, _file_id: &str, _range: Option<Range>) -> Result<Vec<u8>> {
            Err(fake())
        }
        async fn set_trashed(&self, _file_id: &str, _trashed: bool) -> Result<()> {
            Err(fake())
        }
        async fn delete(&self, _file_id: &str) -> Result<()> {
            Err(fake())
        }
    }

    fn fake() -> ProviderError {
        ProviderError::Provider("not implemented by the fake".into())
    }

    #[test]
    fn provider_is_usable_as_a_trait_object() {
        let fake = FakeProvider;
        let provider: &dyn StorageProvider = &fake;
        assert_eq!(provider.kind(), ProviderKind::Google);
    }
}
