//! Google Drive implementation of [`crate::StorageProvider`].
//!
//! This module owns the provider's identity and endpoints. The provider's methods are
//! split across inherent `impl GoogleDrive` blocks so each piece of the API can be
//! reviewed on its own; the single `impl StorageProvider` forwards to them.

mod client;
mod folders;
pub(crate) mod map;
#[cfg(test)]
mod stub;
mod sync;
mod transfer;

use async_trait::async_trait;

use self::map::{DriveFile, FILE_FIELDS, remote_file};
use crate::error::Result;
use crate::provider::{StorageProvider, UploadProgress};
use crate::types::{
    AccountInfo, ChangePage, FolderListing, ProviderKind, Quota, Range, RemoteFile, UploadRequest,
};

pub struct GoogleDrive {
    pub(crate) client: client::Client,
}

impl GoogleDrive {
    pub fn new(access_token: impl Into<String>) -> Self {
        Self::with_client(client::Client::new(
            access_token.into(),
            client::DEFAULT_BASE.into(),
            client::DEFAULT_UPLOAD_BASE.into(),
        ))
    }

    /// Swap a refreshed token in without rebuilding the provider. `drive-oauth` owns the
    /// refresh, so this only has to publish the new value.
    pub fn set_access_token(&self, token: impl Into<String>) {
        self.client.set_access_token(token);
    }

    /// Aim the client at a local stub instead of Google.
    #[cfg(test)]
    pub(crate) fn with_endpoints(access_token: &str, base: &str, upload_base: &str) -> Self {
        Self::with_client(client::Client::with_backoff(
            access_token,
            base,
            upload_base,
            client::TEST_DELAY_BASE,
            client::TEST_DELAY_CAP,
            client::TEST_SLEEP_BUDGET,
        ))
    }

    pub fn kind(&self) -> ProviderKind {
        ProviderKind::Google
    }

    fn with_client(client: client::Client) -> Self {
        Self { client }
    }
}

/// Pure forwarding: every method body lives in the sibling `impl GoogleDrive` blocks above.
#[async_trait]
impl StorageProvider for GoogleDrive {
    fn kind(&self) -> ProviderKind {
        Self::kind(self)
    }

    async fn account_info(&self) -> Result<AccountInfo> {
        GoogleDrive::account_info(self).await
    }

    async fn quota(&self) -> Result<Quota> {
        GoogleDrive::quota(self).await
    }

    async fn ensure_app_folder(&self, name: &str) -> Result<String> {
        GoogleDrive::ensure_app_folder(self, name).await
    }

    async fn create_folder(&self, parent_id: &str, name: &str) -> Result<String> {
        GoogleDrive::create_folder(self, parent_id, name).await
    }

    async fn list_folder(&self, folder_id: &str) -> Result<FolderListing> {
        GoogleDrive::list_folder(self, folder_id).await
    }

    async fn changes(&self, page_token: Option<&str>) -> Result<ChangePage> {
        GoogleDrive::changes(self, page_token).await
    }

    async fn upload(
        &self,
        req: UploadRequest,
        progress: Option<&UploadProgress>,
    ) -> Result<RemoteFile> {
        // The provider reads and validates the file itself; nothing buffers it in between.
        GoogleDrive::upload(
            self,
            &req.parent_id,
            &req.name,
            &req.mime_type,
            &req.source_path,
            req.size_bytes,
            progress,
        )
        .await
    }

    async fn download(&self, file_id: &str, range: Option<Range>) -> Result<Vec<u8>> {
        // The trait hands over an id, but choosing between a download and a Workspace export
        // needs the mime type and `canDownload`, so they are read here.
        // shortcut: one extra GET per download; passing `&RemoteFile` in the trait would drop it.
        let file: DriveFile =
            self.client.get_json(&format!("files/{file_id}"), &[("fields", FILE_FIELDS)]).await?;
        GoogleDrive::download(self, &remote_file(&file), range).await
    }

    async fn set_trashed(&self, file_id: &str, trashed: bool) -> Result<()> {
        GoogleDrive::set_trashed(self, file_id, trashed).await
    }

    async fn delete(&self, file_id: &str) -> Result<()> {
        GoogleDrive::delete(self, file_id).await
    }
}

impl std::fmt::Debug for GoogleDrive {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GoogleDrive").field("client", &self.client).finish()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::stub::{Reply, Stub, TempFile};
    use super::*;

    const UPLOADED: &str = r#"{"id":"new-1","name":"notes.txt","mimeType":"text/plain","sizeBytes":"5","capabilities":{"canDownload":true}}"#;

    #[test]
    fn default_endpoints_are_google() {
        let drive = GoogleDrive::new("token");
        assert_eq!(drive.client.url("files"), "https://www.googleapis.com/drive/v3/files");
        assert_eq!(
            drive.client.upload_url("files"),
            "https://www.googleapis.com/upload/drive/v3/files"
        );
    }

    #[tokio::test]
    async fn with_endpoints_points_the_provider_at_a_stub() {
        let stub = Stub::start(vec![Reply::json(200, r#"{"id":"me"}"#)]).await;
        let drive = GoogleDrive::with_endpoints("token", &stub.base, &stub.base);

        assert_eq!(drive.client.url("files"), format!("{}/files", stub.base));
        assert_eq!(drive.kind(), ProviderKind::Google);
    }

    #[test]
    fn an_absolute_path_bypasses_the_base() {
        let drive = GoogleDrive::new("token");
        assert_eq!(drive.client.url("http://127.0.0.1:1/files"), "http://127.0.0.1:1/files");
    }

    #[test]
    fn debug_never_prints_the_access_token() {
        let rendered = format!("{:?}", GoogleDrive::new("ya29.super-secret"));
        assert!(!rendered.contains("ya29.super-secret"), "{rendered}");
        assert!(rendered.contains("redacted"), "{rendered}");
    }

    /// The forwarding impl has no logic of its own, but it can still be wrong in a way that
    /// only shows here: a mis-resolved `self.upload(..)` would recurse instead of forwarding.
    #[tokio::test]
    async fn the_trait_reaches_the_inherent_methods() {
        let stub = Stub::start(vec![Reply::json(200, r#"{"files":[{"id":"folder-1"}]}"#)]).await;
        let drive = GoogleDrive::with_endpoints("token", &stub.base, &stub.base);
        let provider: &dyn StorageProvider = &drive;

        let found = provider.ensure_app_folder("9drive").await.unwrap();
        assert_eq!(found, "folder-1");
        assert_eq!(stub.request_count(), 1);
        assert_eq!(provider.kind(), ProviderKind::Google);
    }

    /// The forwarding impl used to `fs::read` the whole file into memory. It must now hand the
    /// path and the progress hook straight over, with the provider doing the reading.
    #[tokio::test]
    async fn the_trait_upload_streams_the_file_and_hands_progress_through() {
        let chunks = Stub::start(vec![Reply::json(200, UPLOADED)]).await;
        let meta = Stub::start(vec![
            Reply::empty(200).header("Location", &format!("{}/session", chunks.base)),
        ])
        .await;
        let drive = GoogleDrive::with_endpoints("token", &meta.base, &meta.base);
        let provider: &dyn StorageProvider = &drive;
        let source = TempFile::of_len(5);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let report = {
            let sink = Arc::clone(&seen);
            move |bytes: u64| sink.lock().expect("the progress lock").push(bytes)
        };
        let req = UploadRequest {
            parent_id: "parent-1".into(),
            name: "notes.txt".into(),
            mime_type: "text/plain".into(),
            size_bytes: source.len(),
            source_path: source.path().into(),
        };

        let uploaded = provider.upload(req, Some(&report)).await.expect("upload through the trait");

        assert_eq!(uploaded.id, "new-1");
        assert_eq!(chunks.requests()[0].header("content-range"), Some("bytes 0-4/5"));
        assert_eq!(chunks.requests()[0].body, "xxxxx");
        assert_eq!(*seen.lock().expect("the progress lock"), vec![5]);
    }
}
