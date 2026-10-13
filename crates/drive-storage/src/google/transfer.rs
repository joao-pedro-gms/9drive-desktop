//! Byte movement both ways: the resumable upload, the download and the Workspace export it
//! falls back to, trashing and permanent deletion.
//!
//! The upload owns its own request loop because Drive's resumable protocol is not a retry
//! policy: every byte sent has a position the next request has to agree with.

use reqwest::{Method, StatusCode};
use serde::Serialize;
use serde_json::json;

use crate::error::{ProviderError, Result};
use crate::types::{Range, RemoteFile};

use super::GoogleDrive;
use super::client::read_capped;
use super::map::{DriveFile, FILE_FIELDS, remote_file};

/// Drive spells the root as this alias, not as a real file id.
const ROOT: &str = "root";

/// Drive requires every chunk but the last to be a whole multiple of 256 KiB.
const CHUNK: usize = 256 * 1024;

/// The plan caps a Workspace export at 10 MB; Drive kills a larger one with a confusing
/// error instead of reporting a size that could have been refused beforehand.
/// shortcut: the cap is checked on the finished body, because Drive only reveals the size by
/// starting the export; stream through `read_capped` once exports get large enough to care.
const EXPORT_CAP: usize = 10 * 1024 * 1024;

/// Workspace types that hold no bytes of their own, and the binary each one exports to. A
/// Form or a Script is deliberately absent: Drive has no binary equivalent for either, and
/// naming a guess would only fail at the far end with a message about the wrong type.
const EXPORT_TARGETS: [(&str, &str); 3] = [
    ("application/vnd.google-apps.document", "application/pdf"),
    (
        "application/vnd.google-apps.spreadsheet",
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
    ),
    (
        "application/vnd.google-apps.presentation",
        "application/vnd.openxmlformats-officedocument.presentationml.presentation",
    ),
];

/// `parents` is left out at the root: Drive reads the absence of a parent, not the literal
/// `root`, as "the root".
#[derive(Serialize)]
struct UploadMetadata<'a> {
    name: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    parents: Option<Vec<&'a str>>,
    #[serde(rename = "mimeType")]
    mime_type: &'a str,
}

impl GoogleDrive {
    /// shortcut: the whole payload is already in memory because the trait hands over a
    /// `Vec<u8>`, so each chunk is copied once into the request body; a byte stream (F4)
    /// drops that copy.
    pub async fn upload(
        &self,
        parent_id: &str,
        name: &str,
        data: Vec<u8>,
        mime_type: &str,
    ) -> Result<RemoteFile> {
        let session = self.open_session(parent_id, name, mime_type).await?;
        let total = data.len();
        let mut sent = 0usize;

        loop {
            let end = (sent + CHUNK).min(total);
            // An empty file has no position to send, and Drive spells that one case itself.
            let content_range = if total == 0 {
                "bytes */0".to_string()
            } else {
                format!("bytes {sent}-{}/{total}", end - 1)
            };
            let chunk = self
                .client
                .request(Method::PUT, &session)
                .header("Content-Range", content_range)
                // The bytes are opaque to Drive; the media type lives in the metadata.
                .header("Content-Type", "application/octet-stream")
                .body(data[sent..end].to_vec());
            let response = self.client.send_raw(chunk).await?;

            match response.status() {
                // 308 Resume Incomplete: Drive wants the rest of the file.
                StatusCode::PERMANENT_REDIRECT => {
                    // Every byte is already out, so another 308 cannot be answered by
                    // advancing; without this the loop would re-send the same chunk forever.
                    if end == total {
                        return Err(ProviderError::Provider(
                            "upload finished on a 308 Resume Incomplete".into(),
                        ));
                    }
                    sent = end;
                }
                status if status.is_success() => {
                    let file: DriveFile = serde_json::from_slice(&read_capped(response).await?)
                        .map_err(|err| {
                            ProviderError::Decode(format!("upload reply was not a file ({err})"))
                        })?;
                    return Ok(remote_file(&file));
                }
                status => return Err(upload_failure(status)),
            }
        }
    }

    /// The session URI comes from the `Location` header, and the field mask rides on the
    /// metadata POST because Drive applies it to the file it assembles from the last chunk.
    async fn open_session(&self, parent_id: &str, name: &str, mime_type: &str) -> Result<String> {
        let metadata = UploadMetadata {
            name,
            parents: (parent_id != ROOT).then(|| vec![parent_id]),
            mime_type,
        };
        let url =
            self.client.upload_url(&format!("files?uploadType=resumable&fields={FILE_FIELDS}"));
        let response =
            self.client.send_raw(self.client.request(Method::POST, &url).json(&metadata)).await?;

        let status = response.status();
        if !status.is_success() {
            return Err(upload_failure(status));
        }
        response
            .headers()
            .get("location")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
            .ok_or_else(|| ProviderError::Decode("upload session reply carried no Location".into()))
    }

    /// Workspace documents have no bytes of their own; Drive reports `can_download: false`
    /// for them and `/export` is the only way to get anything out.
    pub async fn download(&self, file: &RemoteFile, range: Option<Range>) -> Result<Vec<u8>> {
        if !file.can_download {
            return self.export(file).await;
        }
        let path = format!("files/{}?alt=media", file.id);
        // 200 and 206 are both answers; the body is the whole file or the asked-for slice.
        let (bytes, _) = self.client.get_bytes(&path, range_header(range).as_deref()).await?;
        Ok(bytes)
    }

    /// Drive's export ignores `Range`, so a ranged read of a Workspace document comes back
    /// whole; that is Drive's behaviour, not a range we drop on the floor.
    ///
    /// shortcut: the cap is checked on the finished body, so an oversized export is downloaded
    /// in full before it is rejected; enforcing it while reading needs a streaming reader.
    async fn export(&self, file: &RemoteFile) -> Result<Vec<u8>> {
        let target = export_target(&file.mime_type)?;
        let path = format!("files/{}/export?mimeType={target}", file.id);
        let (bytes, _) = self.client.get_bytes(&path, None).await?;
        if bytes.len() > EXPORT_CAP {
            return Err(ProviderError::Provider(format!(
                "export of {} is {} bytes, over the {EXPORT_CAP} byte cap",
                file.id,
                bytes.len()
            )));
        }
        Ok(bytes)
    }

    /// Moves the file into the Drive trash, or brings it back out.
    pub async fn set_trashed(&self, file_id: &str, trashed: bool) -> Result<()> {
        self.client
            .patch_json_empty(&format!("files/{file_id}"), &json!({ "trashed": trashed }))
            .await
    }

    /// Permanent: Drive discards the file and its id, and nothing here can undo it. The
    /// recoverable half of a delete is [`GoogleDrive::set_trashed`].
    pub async fn delete(&self, file_id: &str) -> Result<()> {
        self.client.delete(&format!("files/{file_id}")).await
    }
}

/// Both ends are inclusive positions, not a length.
fn range_header(range: Option<Range>) -> Option<String> {
    range.map(|range| format!("bytes={}-{}", range.start(), range.end()))
}

fn export_target(mime_type: &str) -> Result<&'static str> {
    EXPORT_TARGETS
        .iter()
        .find_map(|(workspace, target)| (*workspace == mime_type).then_some(*target))
        .ok_or_else(|| ProviderError::InvalidArgument(format!("no binary export for {mime_type}")))
}

/// `send_raw` maps no status by design — the loop above owns its own attempts, so a silent
/// replay would duplicate bytes — and the codes a caller switches on are mapped here.
fn upload_failure(status: StatusCode) -> ProviderError {
    let message = format!("{status}: resumable upload refused");
    match status {
        StatusCode::UNAUTHORIZED => ProviderError::Unauthorized(message),
        StatusCode::FORBIDDEN => ProviderError::Forbidden(message),
        StatusCode::NOT_FOUND => ProviderError::NotFound(message),
        StatusCode::CONFLICT => ProviderError::Conflict(message),
        StatusCode::TOO_MANY_REQUESTS => ProviderError::RateLimited { message, retry_after: None },
        _ => ProviderError::Provider(message),
    }
}

#[cfg(test)]
mod tests {
    use super::super::stub::{Reply, Stub};
    use super::*;

    const TOKEN: &str = "ya29.test-token";
    const FORBIDDEN: &str = r#"{"error":{"code":403,"message":"Insufficient permissions","errors":[{"reason":"insufficientFilePermissions"}]}}"#;
    const UPLOADED: &str = r#"{"id":"new-1","name":"notes.txt","mimeType":"text/plain","sizeBytes":"5","parents":["parent-1"],"capabilities":{"canDownload":true}}"#;

    fn drive(stub: &Stub) -> GoogleDrive {
        GoogleDrive::with_endpoints(TOKEN, &stub.base, &stub.base)
    }

    fn file(mime_type: &str, can_download: bool) -> RemoteFile {
        RemoteFile {
            id: "f1".into(),
            name: "notes.txt".into(),
            mime_type: mime_type.into(),
            size_bytes: None,
            md5_checksum: None,
            parent_id: None,
            trashed: false,
            modified_at: None,
            can_download,
        }
    }

    /// Drive hands back an absolute session URI, so the chunk responder has to be listening
    /// before the session reply can name it.
    async fn stubs(session_reply: impl FnOnce(&str) -> Reply, chunks: Vec<Reply>) -> (Stub, Stub) {
        let chunks = Stub::start(chunks).await;
        let meta = Stub::start(vec![session_reply(&chunks.base)]).await;
        (meta, chunks)
    }

    fn with_session(base: &str) -> Reply {
        Reply::empty(200).header("Location", &format!("{base}/session"))
    }

    #[tokio::test]
    async fn a_payload_below_one_chunk_is_a_metadata_post_and_a_single_put() {
        let (meta, chunks) = stubs(with_session, vec![Reply::json(200, UPLOADED)]).await;
        let drive = drive(&meta);

        let uploaded = drive
            .upload("parent-1", "notes.txt", b"hello".to_vec(), "text/plain")
            .await
            .expect("upload");

        assert_eq!(uploaded.id, "new-1");
        assert_eq!(uploaded.name, "notes.txt");
        assert_eq!(uploaded.size_bytes, Some(5));
        // Nothing read the capabilities without the mask, so this would be false without it.
        assert!(uploaded.can_download);

        assert_eq!(meta.request_count(), 1);
        let post = &meta.requests()[0];
        assert_eq!(post.method, "POST");
        assert_eq!(post.path, "/files");
        assert_eq!(post.query("uploadType").as_deref(), Some("resumable"));
        assert_eq!(post.query("fields").as_deref(), Some(FILE_FIELDS));
        assert!(post.body.contains(r#""name":"notes.txt""#), "{}", post.body);
        assert!(post.body.contains(r#""parents":["parent-1"]"#), "{}", post.body);
        assert!(post.body.contains(r#""mimeType":"text/plain""#), "{}", post.body);

        assert_eq!(chunks.request_count(), 1);
        let put = &chunks.requests()[0];
        assert_eq!(put.method, "PUT");
        assert_eq!(put.path, "/session");
        assert_eq!(put.header("content-range"), Some("bytes 0-4/5"));
        assert_eq!(put.body, "hello");
    }

    #[tokio::test]
    async fn an_upload_at_the_root_omits_the_parent() {
        let (meta, chunks) = stubs(with_session, vec![Reply::json(200, UPLOADED)]).await;

        drive(&meta).upload(ROOT, "top.txt", b"x".to_vec(), "text/plain").await.expect("upload");

        let body = &meta.requests()[0].body;
        assert!(body.contains(r#""name":"top.txt""#), "{body}");
        assert!(!body.contains("parents"), "{body}");
        assert_eq!(chunks.requests()[0].header("content-range"), Some("bytes 0-0/1"));
    }

    #[tokio::test]
    async fn every_chunk_is_its_own_put_and_a_308_does_not_end_the_upload() {
        let (meta, chunks) = stubs(
            with_session,
            vec![Reply::empty(308), Reply::empty(308), Reply::json(200, UPLOADED)],
        )
        .await;
        let total = CHUNK * 2 + 1024;

        let uploaded = drive(&meta)
            .upload("parent-1", "notes.txt", vec![b'x'; total], "text/plain")
            .await
            .expect("upload");

        assert_eq!(uploaded.id, "new-1");
        assert_eq!(chunks.request_count(), 3);
        let expected = [
            format!("bytes 0-{}/{total}", CHUNK - 1),
            format!("bytes {CHUNK}-{}/{total}", CHUNK * 2 - 1),
            format!("bytes {}-{}/{total}", CHUNK * 2, total - 1),
        ];
        for (index, range) in expected.iter().enumerate() {
            let request = &chunks.requests()[index];
            assert_eq!(request.method, "PUT");
            assert_eq!(request.header("content-range"), Some(range.as_str()));
        }
        // Every chunk but the last is a whole number of 256 KiB, which Drive requires.
        assert_eq!(chunks.requests()[0].body.len(), CHUNK);
        assert_eq!(chunks.requests()[2].body.len(), 1024);
    }

    #[tokio::test]
    async fn an_empty_file_is_uploaded_as_a_zero_length_final_chunk() {
        let (meta, chunks) = stubs(with_session, vec![Reply::json(200, UPLOADED)]).await;

        drive(&meta)
            .upload("parent-1", "empty.txt", Vec::new(), "text/plain")
            .await
            .expect("upload");

        assert_eq!(chunks.request_count(), 1);
        assert_eq!(chunks.requests()[0].header("content-range"), Some("bytes */0"));
    }

    #[tokio::test]
    async fn a_metadata_reply_without_a_location_is_a_decode_error() {
        let stub = Stub::start(vec![Reply::empty(200)]).await;

        let err = drive(&stub)
            .upload("parent-1", "a.txt", b"x".to_vec(), "text/plain")
            .await
            .expect_err("must fail");

        assert_eq!(err.code(), "decode");
    }

    #[tokio::test]
    async fn a_403_on_either_upload_request_is_forbidden_and_reaches_no_chunk() {
        let (meta, chunks) =
            stubs(|_| Reply::json(403, FORBIDDEN), vec![Reply::json(200, UPLOADED)]).await;
        let err = drive(&meta)
            .upload("parent-1", "a.txt", b"x".to_vec(), "text/plain")
            .await
            .unwrap_err();
        assert_eq!(err.code(), "forbidden");
        assert_eq!(chunks.request_count(), 0);

        let (meta, chunks) = stubs(with_session, vec![Reply::json(403, FORBIDDEN)]).await;
        let err = drive(&meta)
            .upload("parent-1", "a.txt", b"x".to_vec(), "text/plain")
            .await
            .unwrap_err();
        assert_eq!(err.code(), "forbidden");
        assert_eq!(chunks.request_count(), 1);
    }

    #[tokio::test]
    async fn a_downloadable_file_is_read_with_alt_media_and_no_range() {
        let stub = Stub::start(vec![Reply::bytes(200, b"whole file".to_vec())]).await;

        let bytes = drive(&stub).download(&file("text/plain", true), None).await.expect("download");

        assert_eq!(bytes, b"whole file");
        let request = &stub.requests()[0];
        assert_eq!(request.method, "GET");
        assert_eq!(request.path, "/files/f1");
        assert_eq!(request.query("alt").as_deref(), Some("media"));
        assert_eq!(request.header("range"), None);
    }

    #[tokio::test]
    async fn a_range_is_sent_inclusive_and_a_206_body_is_the_slice() {
        let stub = Stub::start(vec![
            Reply::bytes(206, b"0123456789".to_vec()).header("Content-Range", "bytes 0-9/100"),
        ])
        .await;

        let range = Range::new(0, 9).expect("range");
        let bytes =
            drive(&stub).download(&file("text/plain", true), Some(range)).await.expect("206");

        assert_eq!(bytes, b"0123456789");
        assert_eq!(stub.requests()[0].header("range"), Some("bytes=0-9"));
    }

    #[tokio::test]
    async fn a_workspace_document_is_exported_and_never_read_with_alt_media() {
        let stub = Stub::start(vec![Reply::bytes(200, b"%PDF-1.7".to_vec())]).await;
        let doc = file("application/vnd.google-apps.document", false);

        assert_eq!(drive(&stub).download(&doc, None).await.expect("export"), b"%PDF-1.7");

        let request = &stub.requests()[0];
        assert_eq!(request.path, "/files/f1/export");
        assert_eq!(request.query("mimeType").as_deref(), Some("application/pdf"));
        assert_eq!(request.query("alt"), None);
    }

    #[test]
    fn every_workspace_type_maps_to_its_binary_equivalent() {
        for (workspace, target) in EXPORT_TARGETS {
            assert_eq!(export_target(workspace).expect("mapped"), target);
        }
        // A fourth entry with no table row would silently stop being asserted above.
        assert_eq!(EXPORT_TARGETS.len(), 3);
    }

    #[tokio::test]
    async fn a_google_form_has_no_binary_export_and_names_its_type() {
        let stub = Stub::start(vec![Reply::json(200, "unused")]).await;

        let err = drive(&stub)
            .download(&file("application/vnd.google-apps.form", false), None)
            .await
            .expect_err("must fail");

        assert_eq!(err.code(), "invalid_argument");
        assert!(err.to_string().contains("application/vnd.google-apps.form"), "{err}");
        assert_eq!(stub.request_count(), 0);
    }

    #[tokio::test]
    async fn an_export_over_the_cap_is_rejected_rather_than_returned() {
        let stub = Stub::start(vec![Reply::bytes(200, vec![b'x'; EXPORT_CAP + 1])]).await;
        let sheet = file("application/vnd.google-apps.spreadsheet", false);

        let err = drive(&stub).download(&sheet, None).await.expect_err("must fail");

        assert!(err.to_string().contains("cap"), "{err}");
        assert_eq!(stub.request_count(), 1);
    }

    /// The export guard has to sit on the finished body because the shared reader stops long
    /// before the plan's 10 MB; it cannot be the cap that enforces it.
    #[tokio::test]
    async fn the_shared_capped_reader_stops_short_of_the_export_cap() {
        let stub = Stub::start(vec![Reply::bytes(200, vec![b'x'; EXPORT_CAP])]).await;
        let response = reqwest::get(&stub.base).await.expect("get");

        assert!(read_capped(response).await.expect("capped read").len() < EXPORT_CAP);
    }

    #[tokio::test]
    async fn trashing_patches_the_flag_and_deleting_sends_a_delete() {
        let stub = Stub::start(vec![Reply::empty(204)]).await;
        let drive = drive(&stub);

        drive.set_trashed("f1", true).await.expect("trash");
        drive.set_trashed("f1", false).await.expect("restore");
        drive.delete("f1").await.expect("delete");

        let requests = stub.requests();
        assert_eq!(requests[0].method, "PATCH");
        assert_eq!(requests[0].path, "/files/f1");
        assert!(requests[0].body.contains(r#""trashed":true"#), "{}", requests[0].body);
        assert!(requests[1].body.contains(r#""trashed":false"#), "{}", requests[1].body);
        assert_eq!(requests[2].method, "DELETE");
        assert_eq!(requests[2].path, "/files/f1");
    }

    #[tokio::test]
    async fn a_403_on_download_trash_and_delete_is_forbidden() {
        let stub = Stub::start(vec![Reply::json(403, FORBIDDEN)]).await;
        let drive = drive(&stub);

        let codes = [
            drive.download(&file("text/plain", true), None).await.unwrap_err(),
            drive.set_trashed("f1", true).await.unwrap_err(),
            drive.delete("f1").await.unwrap_err(),
        ];

        for err in codes {
            assert_eq!(err.code(), "forbidden", "{err}");
        }
        // A plain 403 is not retried by the client.
        assert_eq!(stub.request_count(), 3);
    }
}
