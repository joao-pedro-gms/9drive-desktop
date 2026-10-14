//! The `9drive://` scheme: a video or audio element asks the webview for a byte range and gets
//! it straight from Drive, instead of the frontend pulling a whole file over IPC.
//!
//! The handler never logs a token and never logs file contents (AGENTS.md).

use drive_core::db::{self, SharedConn};
use drive_core::error::Result;
use drive_storage::types::Range;
use rusqlite::OptionalExtension;
use tauri::http::{Request, Response, StatusCode, header};
use tauri::{AppHandle, Manager, Runtime, UriSchemeContext, UriSchemeResponder};

use crate::state::AppState;

pub const SCHEME: &str = "9drive";

/// Registered by [`crate::run`]; the work happens on Tauri's runtime so the webview is never
/// blocked, and the answer is handed back through `responder` rather than a returned future.
pub fn handler<R: Runtime>(
    context: UriSchemeContext<'_, R>,
    request: Request<Vec<u8>>,
    responder: UriSchemeResponder,
) {
    let app = context.app_handle().clone();
    tauri::async_runtime::spawn(async move {
        responder.respond(respond(&app, request).await);
    });
}

async fn respond<R: Runtime>(app: &AppHandle<R>, request: Request<Vec<u8>>) -> Response<Vec<u8>> {
    // `Uri` reparses `9drive://file/x` as authority `file` and path `/x`, so the raw string is
    // what still carries the route.
    let url = request.uri().to_string();
    let Some(file_id) = file_id(&url) else {
        return not_found();
    };
    let state = app.state::<AppState>();
    let media = match find(&state.conn(), file_id) {
        Ok(Some(media)) => media,
        Ok(None) => return not_found(),
        Err(err) => return failed(err.code()),
    };

    let range = parse_range(request.headers().get(header::RANGE), media.size_bytes);
    let provider = match state.provider(media.account_id).await {
        Ok(provider) => provider,
        Err(err) => return failed(err.code()),
    };
    match provider.download(file_id, range).await {
        Ok(body) => media_response(range, media.size_bytes, content_type(&media.name), body),
        Err(err) => failed(err.code()),
    }
}

/// What the protocol needs out of the local mirror of Drive.
struct Media {
    account_id: i64,
    name: String,
    size_bytes: Option<u64>,
}

/// One indexed lookup: `drive-core` owns the table and has no lookup keyed on a Drive id, and
/// re-deriving the whole `FileItem` here to throw most of it away would be worse.
fn find(conn: &SharedConn, drive_file_id: &str) -> Result<Option<Media>> {
    db::with_conn(conn, |c| {
        Ok(c.query_row(
            "SELECT account_id, name, size_bytes FROM files WHERE drive_file_id = ?1",
            [drive_file_id],
            |row| {
                Ok(Media {
                    account_id: row.get(0)?,
                    name: row.get(1)?,
                    // SQLite has no unsigned integers, and a size can be absent for a
                    // Workspace document that was never sized.
                    size_bytes: row.get::<_, Option<i64>>(2)?.and_then(|s| u64::try_from(s).ok()),
                })
            },
        )
        .optional()?)
    })
}

/// `9drive://file/<driveFileId>`, with `9drive://media/<driveFileId>` as the second spelling.
/// The last two segments decide, because the webview rewrites a custom scheme differently per
/// platform: Windows and Linux hand over `9drive://localhost/file/<id>`, macOS `9drive://file/<id>`.
fn file_id(url: &str) -> Option<&str> {
    let path = url.split_once("://").map_or(url, |(_, rest)| rest);
    let mut segments = path.split(['?', '#']).next()?.split('/').filter(|s| !s.is_empty());
    let id = segments.next_back()?;
    let route = segments.next_back()?;
    matches!(route, "file" | "media").then_some(id)
}

/// Only `bytes=first-last` and `bytes=first-` are honoured, which is all a media element asks
/// for; anything else is ignored and the whole file comes back with a 200.
/// shortcut: suffix (`bytes=-500`) and multi-range requests are not split, upgrade if a
/// browser ever sends one.
fn parse_range(header: Option<&header::HeaderValue>, size: Option<u64>) -> Option<Range> {
    let spec = header?.to_str().ok()?.trim().strip_prefix("bytes=")?;
    let (first, last) = spec.split_once('-')?;
    let start = first.trim().parse().ok()?;
    // An open-ended range has to be closed against the size the sync engine recorded.
    let end = match last.trim() {
        "" => size?.checked_sub(1)?,
        bound => bound.parse().ok()?,
    };
    Range::new(start, end).ok()
}

/// From the file name, not the stored mime type: Drive sends the type only for files it knows
/// and a preview must work for whatever the user uploaded.
fn content_type(name: &str) -> &'static str {
    let extension =
        name.rsplit_once('.').map_or("", |(_, extension)| extension).to_ascii_lowercase();
    match extension.as_str() {
        "mp4" | "m4v" => "video/mp4",
        "webm" => "video/webm",
        "ogv" => "video/ogg",
        "mkv" => "video/x-matroska",
        "mov" => "video/quicktime",
        "mp3" => "audio/mpeg",
        "m4a" => "audio/mp4",
        "oga" | "ogg" => "audio/ogg",
        "wav" => "audio/wav",
        "flac" => "audio/flac",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "svg" => "image/svg+xml",
        "pdf" => "application/pdf",
        _ => "application/octet-stream",
    }
}

fn media_response(
    range: Option<Range>,
    total: Option<u64>,
    content_type: &str,
    body: Vec<u8>,
) -> Response<Vec<u8>> {
    let mut response = Response::builder()
        .status(if range.is_some() { StatusCode::PARTIAL_CONTENT } else { StatusCode::OK })
        .header(header::CONTENT_TYPE, content_type)
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_LENGTH, body.len());
    if let Some(range) = range {
        let total = total.map_or_else(|| "*".to_string(), |total| total.to_string());
        response = response.header(
            header::CONTENT_RANGE,
            format!("bytes {}-{}/{total}", range.start(), range.end()),
        );
    }
    response.body(body).expect("a status and header value always build")
}

fn not_found() -> Response<Vec<u8>> {
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .body(Vec::new())
        .expect("a status alone always builds")
}

/// Only the stable code is logged: a provider message can echo a response body back.
fn failed(code: &str) -> Response<Vec<u8>> {
    tracing::warn!(code, "9drive:// request failed");
    Response::builder()
        .status(StatusCode::INTERNAL_SERVER_ERROR)
        .body(Vec::new())
        .expect("a status alone always builds")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header_value(value: &str) -> header::HeaderValue {
        header::HeaderValue::from_str(value).expect("a valid header value")
    }

    #[test]
    fn both_routes_resolve_the_file_id_in_every_platform_spelling() {
        for url in [
            "9drive://file/drive-1",
            "9drive://media/drive-1",
            // Windows and Linux rewrite the scheme to an http origin and back.
            "9drive://localhost/file/drive-1",
            "9drive://localhost/media/drive-1",
            "9drive:/file/drive-1",
            "9drive://file/drive-1?token=ignored",
        ] {
            assert_eq!(file_id(url), Some("drive-1"), "{url}");
        }
    }

    #[test]
    fn a_url_that_is_not_a_media_route_resolves_to_nothing() {
        for url in [
            "9drive://folder/drive-1",
            "9drive://file",
            "9drive://file/",
            "9drive://drive-1",
            "9drive://",
            "",
        ] {
            assert_eq!(file_id(url), None, "{url:?}");
        }
    }

    #[test]
    fn a_plain_range_becomes_a_partial_response() {
        let range = parse_range(Some(&header_value("bytes=0-1023")), Some(4096)).unwrap();
        assert_eq!((range.start(), range.end()), (0, 1023));
    }

    #[test]
    fn an_open_ended_range_is_closed_against_the_recorded_size() {
        let range = parse_range(Some(&header_value("bytes=1024-")), Some(4096)).unwrap();
        assert_eq!((range.start(), range.end()), (1024, 4095));
    }

    #[test]
    fn a_range_that_cannot_be_answered_falls_back_to_the_whole_file() {
        let none: Option<Range> = None;
        assert_eq!(none, parse_range(None, Some(4096)));
        // No size means an open-ended range has no end.
        assert_eq!(none, parse_range(Some(&header_value("bytes=1024-")), None));
        // The range is unusable rather than a server error.
        assert_eq!(none, parse_range(Some(&header_value("bytes=1023-0")), Some(4096)));
        assert_eq!(none, parse_range(Some(&header_value("bytes=-500")), Some(4096)));
        assert_eq!(none, parse_range(Some(&header_value("items=0-10")), Some(4096)));
        assert_eq!(none, parse_range(Some(&header_value("bytes=0-abc")), Some(4096)));
        assert_eq!(none, parse_range(Some(&header_value("bytes=0-")), Some(0)));
    }

    #[test]
    fn a_range_answers_206_and_a_whole_file_answers_200() {
        let partial = media_response(
            parse_range(Some(&header_value("bytes=0-3")), Some(10)),
            Some(10),
            "video/mp4",
            b"0123".to_vec(),
        );
        assert_eq!(partial.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(partial.headers()[header::ACCEPT_RANGES], "bytes");
        assert_eq!(partial.headers()[header::CONTENT_TYPE], "video/mp4");
        assert_eq!(partial.headers()[header::CONTENT_LENGTH], "4");
        assert_eq!(partial.headers()[header::CONTENT_RANGE], "bytes 0-3/10");

        let whole = media_response(None, Some(10), "video/mp4", b"0123456789".to_vec());
        assert_eq!(whole.status(), StatusCode::OK);
        assert_eq!(whole.headers()[header::ACCEPT_RANGES], "bytes");
        assert_eq!(whole.headers()[header::CONTENT_LENGTH], "10");
        assert!(!whole.headers().contains_key(header::CONTENT_RANGE));
    }

    #[test]
    fn an_unknown_total_answers_a_partial_content_range_of_star() {
        let response = media_response(
            parse_range(Some(&header_value("bytes=0-3")), None),
            None,
            "audio/mpeg",
            b"0123".to_vec(),
        );
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(response.headers()[header::CONTENT_RANGE], "bytes 0-3/*");
    }

    #[test]
    fn a_malformed_url_is_a_404_and_a_failure_is_a_500() {
        assert_eq!(not_found().status(), StatusCode::NOT_FOUND);
        assert_eq!(failed("unauthorized").status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn content_type_comes_from_the_extension_and_falls_back_to_octet_stream() {
        assert_eq!(content_type("holiday.MP4"), "video/mp4");
        assert_eq!(content_type("song.flac"), "audio/flac");
        assert_eq!(content_type("scan.pdf"), "application/pdf");
        assert_eq!(content_type("archive.7z"), "application/octet-stream");
        assert_eq!(content_type("no-extension"), "application/octet-stream");
    }

    #[test]
    fn find_reads_the_account_name_and_size_the_response_needs() {
        let conn = db::open_in_memory().expect("in-memory database");
        let account_id =
            drive_core::repos::accounts::create(&conn, "google", "Test", "a@example.com")
                .unwrap()
                .id;
        conn.execute(
            "INSERT INTO files (account_id, drive_file_id, name, mime_type, size_bytes, starred,
                                archived, created_at, updated_at)
             VALUES (?1, 'drive-1', 'holiday.mp4', 'video/mp4', 4096, 0, 0, 0, 0)",
            [account_id],
        )
        .expect("file row");

        let shared = db::shared(conn);
        let media = find(&shared, "drive-1").unwrap().expect("the mirrored file");
        assert_eq!(media.account_id, account_id);
        assert_eq!(media.name, "holiday.mp4");
        assert_eq!(media.size_bytes, Some(4096));

        assert!(find(&shared, "drive-2").unwrap().is_none(), "an unknown id is not an error");
    }
}
