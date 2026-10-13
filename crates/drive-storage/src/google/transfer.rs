//! Byte movement both ways: the resumable upload, the download and the Workspace export it
//! falls back to, trashing and permanent deletion.
//!
//! The upload owns its own request loop because Drive's resumable protocol is not a retry
//! policy: every byte sent has a position the next request has to agree with.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};

use reqwest::{Method, Response, StatusCode};
use serde::Serialize;
use serde_json::json;

use crate::error::{ProviderError, Result};
use crate::provider::UploadProgress;
use crate::types::{Range, RemoteFile};

use super::GoogleDrive;
use super::client::read_capped;
use super::map::{DriveFile, FILE_FIELDS, remote_file};

/// Drive spells the root as this alias, not as a real file id.
const ROOT: &str = "root";

/// Drive requires every chunk but the last to be a whole multiple of 256 KiB.
const CHUNK: usize = 256 * 1024;

/// shortcut: four tries per chunk and no pause between them; add the client's backoff once a
/// real 5xx burst shows that retrying a chunk immediately is not enough.
const MAX_CHUNK_ATTEMPTS: u32 = 4;

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
    /// Streams `source_path` to Drive one chunk at a time and never holds more than
    /// [`CHUNK`] bytes of it in memory. `size_bytes` is checked against the file before any
    /// request goes out, and `progress` is told the cumulative bytes Drive has confirmed.
    pub async fn upload(
        &self,
        parent_id: &str,
        name: &str,
        mime_type: &str,
        source_path: &str,
        size_bytes: u64,
        progress: Option<&UploadProgress>,
    ) -> Result<RemoteFile> {
        let total = checked_length(source_path, size_bytes)?;
        let session = self.open_session(parent_id, name, mime_type).await?;

        // Where the next chunk starts, and how many attempts in a row have failed to move it.
        let mut offset = 0u64;
        let mut attempts = 0u32;
        loop {
            let end = (offset + CHUNK as u64).min(total);
            // An empty file has no position to send, and Drive spells that one case itself.
            let content_range = if total == 0 {
                "bytes */0".to_string()
            } else {
                format!("bytes {offset}-{}/{total}", end - 1)
            };
            let body = read_chunk(source_path, offset, (end - offset) as usize)?;

            match self.send_chunk(&session, content_range, body).await? {
                ChunkOutcome::Done(file) => {
                    report(progress, total);
                    return Ok(file);
                }
                // Drive's word wins, even where it disagrees with the range just sent.
                ChunkOutcome::Resume(Some(next)) if next > offset && next < total => {
                    offset = next;
                    attempts = 0;
                    report(progress, offset);
                }
                // No Range header: Drive holds none of this chunk, so the offset we last
                // confirmed stands and the same range goes out again.
                ChunkOutcome::Resume(None) => {
                    attempts += 1;
                    if attempts >= MAX_CHUNK_ATTEMPTS {
                        return Err(stalled(offset, total));
                    }
                }
                // Drive named an offset this upload cannot reach from here, so there is no
                // range left to send that would satisfy it.
                ChunkOutcome::Resume(Some(_)) => return Err(stalled(offset, total)),
                // After a failure Drive's offset is unknown, so the retry starts where we
                // left off rather than from zero.
                //
                // `shortcut:` Drive may have taken the chunk whose reply we lost, and then
                // rejects the repeated offset with a 400 that ends the upload. The caller's
                // whole-file retry covers it; probe the session URI for its current offset
                // (`GET` returns a 308 with `Range`) when that stops being good enough.
                ChunkOutcome::Retry(err) => {
                    attempts += 1;
                    if attempts >= MAX_CHUNK_ATTEMPTS {
                        return Err(err);
                    }
                }
            }
        }
    }

    /// One chunk PUT. A resumable answer or a failure worth retrying is an outcome rather than
    /// an error, because only the caller's loop knows which offset the retry belongs at.
    async fn send_chunk(
        &self,
        session: &str,
        content_range: String,
        body: Vec<u8>,
    ) -> Result<ChunkOutcome> {
        let put = self
            .client
            .request(Method::PUT, session)
            .header("Content-Range", content_range)
            // The bytes are opaque to Drive; the media type lives in the metadata.
            .header("Content-Type", "application/octet-stream")
            .body(body);

        let response = match self.client.send_raw(put).await {
            Ok(response) => response,
            Err(err) if matches!(err, ProviderError::Transport(_)) => {
                return Ok(ChunkOutcome::Retry(err));
            }
            Err(err) => return Err(err),
        };

        match response.status() {
            // 308 Resume Incomplete: Drive wants the rest of the file.
            StatusCode::PERMANENT_REDIRECT => Ok(ChunkOutcome::Resume(resume_offset(&response))),
            status if status.is_success() => {
                let file: DriveFile = serde_json::from_slice(&read_capped(response).await?)
                    .map_err(|err| {
                        ProviderError::Decode(format!("upload reply was not a file ({err})"))
                    })?;
                Ok(ChunkOutcome::Done(remote_file(&file)))
            }
            status if retryable(status) => Ok(ChunkOutcome::Retry(upload_failure(status))),
            status => Err(upload_failure(status)),
        }
    }

    /// The session URI comes from the `Location` header, and the field mask rides on the
    /// metadata POST because Drive applies it to the file it assembles from the last chunk.
    ///
    /// `shortcut:` `send_raw` never retries, so this POST gets exactly one attempt and a
    /// transient 5xx fails the whole file. The upload queue's `max_attempts` covers it today;
    /// give `Client` a raw-response retry path when the queue stops re-routing retries.
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

/// What one chunk PUT answered.
enum ChunkOutcome {
    /// Drive assembled the file.
    Done(RemoteFile),
    /// `Some` is where Drive says to resume, absent when the reply carried no `Range`.
    Resume(Option<u64>),
    /// A failure worth retrying, carrying the error to report once the attempts run out.
    Retry(ProviderError),
}

/// The file's real length, and the check that the caller's size describes this file: a
/// mismatch means the bytes about to go out are not the ones that were announced.
fn checked_length(path: &str, size_bytes: u64) -> Result<u64> {
    let file = File::open(path).map_err(|err| local_io(path, &err))?;
    let length = file.metadata().map_err(|err| local_io(path, &err))?.len();
    if length != size_bytes {
        return Err(ProviderError::InvalidArgument(format!(
            "{path} is {length} bytes, not the {size_bytes} announced for it"
        )));
    }
    Ok(length)
}

/// One chunk, re-opened and positioned for the resume, so nothing bigger than [`CHUNK`] is
/// ever resident and a retry never leans on a handle left over from a failed request.
/// shortcut: `File` blocks the runtime worker on the read; move to `tokio::fs` when uploads
/// get large enough for a stalled worker to matter (the workspace tokio has no `fs` feature).
fn read_chunk(path: &str, offset: u64, len: usize) -> Result<Vec<u8>> {
    let mut file = File::open(path).map_err(|err| local_io(path, &err))?;
    file.seek(SeekFrom::Start(offset)).map_err(|err| local_io(path, &err))?;
    let mut body = vec![0u8; len];
    file.read_exact(&mut body).map_err(|err| local_io(path, &err))?;
    Ok(body)
}

/// Drive's own count of what it holds, `Range: bytes=0-<last>` with an inclusive end,
/// answered as the offset the next chunk has to start at.
fn resume_offset(response: &Response) -> Option<u64> {
    let range = response.headers().get("range")?.to_str().ok()?;
    let last = range.trim().strip_prefix("bytes=")?.split_once('-')?.1;
    let last: u64 = last.trim().parse().ok()?;
    last.checked_add(1)
}

/// What `Client` treats as worth retrying, minus the per-user 403 whose body this loop does
/// not read: a chunk PUT that comes back forbidden is a permission problem, not a rate limit.
fn retryable(status: StatusCode) -> bool {
    status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error()
}

fn stalled(offset: u64, total: u64) -> ProviderError {
    ProviderError::Provider(format!(
        "upload stalled on a 308 Resume Incomplete at {offset} of {total}"
    ))
}

fn report(progress: Option<&UploadProgress>, bytes: u64) {
    if let Some(report) = progress {
        report(bytes);
    }
}

/// The path belongs in the message; the file's contents never do.
fn local_io(path: &str, err: &std::io::Error) -> ProviderError {
    ProviderError::LocalIo(format!("{path}: {err}"))
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
    use std::sync::{Arc, Mutex};

    use super::super::stub::{Reply, Request, Stub, TempFile};
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

    /// The `Content-Range` of every chunk that reached the stub, verbatim.
    fn ranges(stub: &Stub) -> Vec<String> {
        stub.requests()
            .iter()
            .map(|request| request.header("content-range").unwrap_or_default().to_string())
            .collect()
    }

    /// The positions those headers claim, as (start, end inclusive).
    fn spans(stub: &Stub) -> Vec<(u64, u64)> {
        stub.requests()
            .iter()
            .map(|request: &Request| {
                let span = request.header("content-range").expect("a Content-Range");
                let (start, end) = span
                    .strip_prefix("bytes ")
                    .expect("bytes prefix")
                    .split_once('-')
                    .expect("a span");
                let end = end.split_once('/').expect("a total").0;
                (start.parse().expect("a start"), end.parse().expect("an end"))
            })
            .collect()
    }

    /// Collects the cumulative byte counts the upload reports. The hook has to be `'static`,
    /// so it owns a clone of the sink rather than borrowing it.
    fn recorder(sink: Arc<Mutex<Vec<u64>>>) -> impl Fn(u64) + Send + Sync {
        move |bytes| sink.lock().expect("the progress lock").push(bytes)
    }

    #[tokio::test]
    async fn a_payload_below_one_chunk_is_a_metadata_post_and_a_single_put() {
        let (meta, chunks) = stubs(with_session, vec![Reply::json(200, UPLOADED)]).await;
        let drive = drive(&meta);
        let source = TempFile::of_len(5);

        let uploaded = drive
            .upload("parent-1", "notes.txt", "text/plain", source.path(), source.len(), None)
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
        // Read from the file, not from a caller that no longer hands over any bytes.
        assert_eq!(put.body, "xxxxx");
    }

    #[tokio::test]
    async fn an_upload_at_the_root_omits_the_parent() {
        let (meta, chunks) = stubs(with_session, vec![Reply::json(200, UPLOADED)]).await;
        let source = TempFile::of_len(1);

        drive(&meta)
            .upload(ROOT, "top.txt", "text/plain", source.path(), source.len(), None)
            .await
            .expect("upload");

        let body = &meta.requests()[0].body;
        assert!(body.contains(r#""name":"top.txt""#), "{body}");
        assert!(!body.contains("parents"), "{body}");
        assert_eq!(chunks.requests()[0].header("content-range"), Some("bytes 0-0/1"));
    }

    #[tokio::test]
    async fn every_chunk_is_its_own_put_and_a_308_does_not_end_the_upload() {
        let total = CHUNK * 2 + 1024;
        let (meta, chunks) = stubs(
            with_session,
            vec![
                // Drive counts what it holds in a `Range`, so a 308 advances only that far.
                Reply::empty(308).header("Range", &format!("bytes=0-{}", CHUNK - 1)),
                Reply::empty(308).header("Range", &format!("bytes=0-{}", CHUNK * 2 - 1)),
                Reply::json(200, UPLOADED),
            ],
        )
        .await;
        let source = TempFile::of_len(total);

        let uploaded = drive(&meta)
            .upload("parent-1", "notes.txt", "text/plain", source.path(), source.len(), None)
            .await
            .expect("upload");

        assert_eq!(uploaded.id, "new-1");
        assert_eq!(chunks.request_count(), 3);
        assert_eq!(
            ranges(&chunks),
            [
                format!("bytes 0-{}/{total}", CHUNK - 1),
                format!("bytes {CHUNK}-{}/{total}", CHUNK * 2 - 1),
                format!("bytes {}-{}/{total}", CHUNK * 2, total - 1),
            ]
        );

        // The spans tile 0..total once each: no gap for Drive to wait on, no byte sent twice.
        let spans = spans(&chunks);
        assert_eq!(spans.first().copied(), Some((0, CHUNK as u64 - 1)));
        assert_eq!(spans.last().copied(), Some((total as u64 - 1024, total as u64 - 1)));
        assert!(spans.windows(2).all(|pair| pair[0].1 + 1 == pair[1].0), "{spans:?}");
        // Every chunk but the last is a whole number of 256 KiB, which Drive requires.
        let lengths: Vec<u64> = spans.iter().map(|(start, end)| end - start + 1).collect();
        assert!(
            lengths[..lengths.len() - 1].iter().all(|len| len % CHUNK as u64 == 0),
            "{lengths:?}"
        );
        assert_eq!(chunks.requests()[2].body.len(), 1024);
    }

    #[tokio::test]
    async fn an_empty_file_is_uploaded_as_a_zero_length_final_chunk() {
        let (meta, chunks) = stubs(with_session, vec![Reply::json(200, UPLOADED)]).await;
        let source = TempFile::of_len(0);

        let uploaded = drive(&meta)
            .upload("parent-1", "empty.txt", "text/plain", source.path(), source.len(), None)
            .await
            .expect("upload");

        assert_eq!(uploaded.id, "new-1");
        assert_eq!(chunks.request_count(), 1);
        assert_eq!(chunks.requests()[0].header("content-range"), Some("bytes */0"));
    }

    #[tokio::test]
    async fn progress_is_monotonic_and_finishes_on_the_exact_total() {
        let total = CHUNK * 2 + 7;
        let (meta, chunks) = stubs(
            with_session,
            vec![
                Reply::empty(308).header("Range", &format!("bytes=0-{}", CHUNK - 1)),
                Reply::empty(308).header("Range", &format!("bytes=0-{}", CHUNK * 2 - 1)),
                Reply::json(200, UPLOADED),
            ],
        )
        .await;
        let source = TempFile::of_len(total);
        let seen = Arc::new(Mutex::new(Vec::new()));

        drive(&meta)
            .upload(
                "parent-1",
                "notes.txt",
                "text/plain",
                source.path(),
                source.len(),
                Some(&recorder(Arc::clone(&seen))),
            )
            .await
            .expect("upload");

        let seen = seen.lock().expect("the progress lock").clone();
        // Once per chunk, ending on the total, and never going backwards.
        assert_eq!(seen, vec![CHUNK as u64, CHUNK as u64 * 2, total as u64]);
        assert!(seen.windows(2).all(|pair| pair[0] <= pair[1]), "{seen:?}");
        assert_eq!(seen.len(), chunks.request_count());
    }

    #[tokio::test]
    async fn a_dropped_chunk_resumes_from_the_offset_drive_reports() {
        let total = CHUNK * 2;
        let (meta, chunks) = stubs(
            with_session,
            vec![
                Reply::dropped(),
                Reply::empty(308).header("Range", "bytes=0-999"),
                Reply::json(200, UPLOADED),
            ],
        )
        .await;
        let source = TempFile::of_len(total);

        let uploaded = drive(&meta)
            .upload("parent-1", "notes.txt", "text/plain", source.path(), source.len(), None)
            .await
            .expect("upload");

        assert_eq!(uploaded.id, "new-1");
        // Drive's own offset is where the next chunk starts, so the second chunk goes out
        // twice at most and the first one is never sent again.
        assert_eq!(
            ranges(&chunks),
            [
                format!("bytes 0-{}/{total}", CHUNK - 1),
                format!("bytes 0-{}/{total}", CHUNK - 1),
                format!("bytes 1000-{}/{total}", 1000 + CHUNK - 1),
            ]
        );
        assert_eq!(chunks.requests()[2].body.len(), CHUNK);
    }

    #[tokio::test]
    async fn a_range_header_that_contradicts_the_chunk_is_obeyed_anyway() {
        let total = CHUNK + 1024;
        let (meta, chunks) = stubs(
            with_session,
            vec![Reply::empty(308).header("Range", "bytes=0-1023"), Reply::json(200, UPLOADED)],
        )
        .await;
        let source = TempFile::of_len(total);

        drive(&meta)
            .upload("parent-1", "notes.txt", "text/plain", source.path(), source.len(), None)
            .await
            .expect("upload");

        // A whole 256 KiB was sent, and Drive says it holds 1024 bytes of it. Drive wins.
        assert_eq!(
            ranges(&chunks),
            [format!("bytes 0-{}/{total}", CHUNK - 1), format!("bytes 1024-{}/{total}", total - 1)]
        );
        assert_eq!(chunks.requests()[1].body.len(), total - 1024);
    }

    #[tokio::test]
    async fn a_chunk_that_keeps_failing_stops_at_the_attempt_cap() {
        let (meta, chunks) = stubs(with_session, vec![Reply::dropped()]).await;
        let source = TempFile::of_len(5);

        let err = drive(&meta)
            .upload("parent-1", "a.txt", "text/plain", source.path(), source.len(), None)
            .await
            .expect_err("must fail");

        assert_eq!(err.code(), "transport");
        assert_eq!(chunks.request_count(), MAX_CHUNK_ATTEMPTS as usize);
    }

    #[tokio::test]
    async fn a_308_that_never_advances_cannot_loop_forever() {
        let (meta, chunks) = stubs(with_session, vec![Reply::empty(308)]).await;
        let source = TempFile::of_len(5);

        let err = drive(&meta)
            .upload("parent-1", "a.txt", "text/plain", source.path(), source.len(), None)
            .await
            .expect_err("must fail");

        assert_eq!(err.code(), "provider");
        assert!(err.to_string().contains("308"), "{err}");
        assert_eq!(chunks.request_count(), MAX_CHUNK_ATTEMPTS as usize);
    }

    #[tokio::test]
    async fn a_308_naming_an_offset_outside_the_upload_is_refused() {
        let total = CHUNK;
        let (meta, chunks) = stubs(
            with_session,
            vec![
                Reply::empty(308).header("Range", &format!("bytes=0-{}", total * 4)),
                Reply::json(200, UPLOADED),
            ],
        )
        .await;
        let source = TempFile::of_len(total);

        let err = drive(&meta)
            .upload("parent-1", "a.txt", "text/plain", source.path(), source.len(), None)
            .await
            .expect_err("must fail");

        assert_eq!(err.code(), "provider");
        assert_eq!(chunks.request_count(), 1);
    }

    #[tokio::test]
    async fn a_size_that_disagrees_with_the_file_is_refused_before_any_request() {
        let (meta, chunks) = stubs(with_session, vec![Reply::json(200, UPLOADED)]).await;
        let source = TempFile::of_len(10);

        let err = drive(&meta)
            .upload("parent-1", "a.txt", "text/plain", source.path(), source.len() + 1, None)
            .await
            .expect_err("must fail");

        assert_eq!(err.code(), "invalid_argument");
        // The path names the file that is not the one that was announced; nothing else does.
        assert!(err.to_string().contains(source.path()), "{err}");
        assert_eq!(meta.request_count(), 0);
        assert_eq!(chunks.request_count(), 0);
    }

    #[tokio::test]
    async fn a_missing_source_file_is_a_local_io_error_rather_than_a_panic() {
        let (meta, chunks) = stubs(with_session, vec![Reply::json(200, UPLOADED)]).await;
        let missing = TempFile::absent();

        let err = drive(&meta)
            .upload("parent-1", "a.txt", "text/plain", missing.path(), 10, None)
            .await
            .expect_err("must fail");

        assert_eq!(err.code(), "local_io");
        assert_eq!(meta.request_count(), 0);
        assert_eq!(chunks.request_count(), 0);
    }

    #[tokio::test]
    async fn a_metadata_reply_without_a_location_is_a_decode_error() {
        let stub = Stub::start(vec![Reply::empty(200)]).await;
        let source = TempFile::of_len(1);

        let err = drive(&stub)
            .upload("parent-1", "a.txt", "text/plain", source.path(), source.len(), None)
            .await
            .expect_err("must fail");

        assert_eq!(err.code(), "decode");
    }

    #[tokio::test]
    async fn a_403_on_either_upload_request_is_forbidden_and_reaches_no_chunk() {
        let source = TempFile::of_len(1);
        let (meta, chunks) =
            stubs(|_| Reply::json(403, FORBIDDEN), vec![Reply::json(200, UPLOADED)]).await;
        let err = drive(&meta)
            .upload("parent-1", "a.txt", "text/plain", source.path(), source.len(), None)
            .await
            .unwrap_err();
        assert_eq!(err.code(), "forbidden");
        assert_eq!(chunks.request_count(), 0);

        let (meta, chunks) = stubs(with_session, vec![Reply::json(403, FORBIDDEN)]).await;
        let err = drive(&meta)
            .upload("parent-1", "a.txt", "text/plain", source.path(), source.len(), None)
            .await
            .unwrap_err();
        assert_eq!(err.code(), "forbidden");
        // A forbidden chunk is a permission problem, not something to retry.
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
