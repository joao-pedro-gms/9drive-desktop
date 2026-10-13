//! The HTTP layer every Google Drive call goes through: bearer auth, retry with backoff,
//! and the translation from Drive's JSON error envelope into [`ProviderError`].

use std::fmt;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use reqwest::{Method, RequestBuilder, Response, StatusCode};
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::time::sleep;

use crate::error::{ProviderError, Result};

/// The API roots. Tests point both at a local stub.
pub(crate) const DEFAULT_BASE: &str = "https://www.googleapis.com/drive/v3";
pub(crate) const DEFAULT_UPLOAD_BASE: &str = "https://www.googleapis.com/upload/drive/v3";

#[cfg(test)]
pub(crate) const TEST_DELAY_BASE: Duration = Duration::from_millis(1);
#[cfg(test)]
pub(crate) const TEST_DELAY_CAP: Duration = Duration::from_millis(5);
#[cfg(test)]
pub(crate) const TEST_SLEEP_BUDGET: Duration = Duration::from_millis(50);

/// Drive reports per-user limits as 403 with one of these reasons, never as 429.
const RATE_LIMIT_REASONS: [&str; 2] = ["rateLimitExceeded", "userRateLimitExceeded"];

/// A 5xx from an intermediary can carry anything, so error bodies are read a cap at a
/// time instead of being buffered whole.
/// shortcut: 64 KiB is far past Drive's envelope; raise only if a real body needs more.
const ERROR_BODY_CAP: usize = 64 * 1024;

/// shortcut: one call plus four retries; raise when Drive's own retry window outgrows it.
const MAX_ATTEMPTS: u32 = 5;

/// `Retry-After` is an untrusted hint from a server that may be lying.
/// shortcut: 30 s ceiling; a longer hint should reach the caller instead of a sleep.
const RETRY_AFTER_CAP: Duration = Duration::from_secs(30);

/// A response the caller still has to decode, or an error whose body this layer already
/// read. Errors never come back as a `Response`, because deciding whether a 403 is a rate
/// limit requires reading the body that maps it.
enum Outcome {
    Body(Response),
    Failed(ProviderError),
}

/// Exponential backoff, bounded per sleep and across one call's retries.
#[derive(Clone, Copy)]
struct Backoff {
    base: Duration,
    cap: Duration,
    budget: Duration,
}

impl Backoff {
    const DEFAULT: Self = Self {
        base: Duration::from_millis(250),
        cap: Duration::from_secs(8),
        budget: Duration::from_secs(20),
    };

    fn delay(&self, attempt: u32) -> Duration {
        let factor = 1u32 << attempt.saturating_sub(1).min(16);
        self.base.saturating_mul(factor).min(self.cap)
    }
}

pub(crate) struct Client {
    http: reqwest::Client,
    base: String,
    upload_base: String,
    access_token: Arc<RwLock<String>>,
    backoff: Backoff,
}

// The token must never reach a log line, so Debug is written by hand.
impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Client")
            .field("base", &self.base)
            .field("upload_base", &self.upload_base)
            .field("access_token", &"<redacted>")
            .finish()
    }
}

impl Client {
    pub(crate) fn new(access_token: String, base: String, upload_base: String) -> Self {
        Self::build(access_token, base, upload_base, Backoff::DEFAULT)
    }

    #[cfg(test)]
    pub(crate) fn with_backoff(
        access_token: &str,
        base: &str,
        upload_base: &str,
        delay_base: Duration,
        delay_cap: Duration,
        sleep_budget: Duration,
    ) -> Self {
        Self::build(
            access_token.into(),
            base.into(),
            upload_base.into(),
            Backoff { base: delay_base, cap: delay_cap, budget: sleep_budget },
        )
    }

    fn build(access_token: String, base: String, upload_base: String, backoff: Backoff) -> Self {
        Self {
            http: reqwest::Client::new(),
            base,
            upload_base,
            access_token: Arc::new(RwLock::new(access_token)),
            backoff,
        }
    }

    pub(crate) fn set_access_token(&self, token: impl Into<String>) {
        // A poisoned lock still holds a usable value, so recovering beats dropping the swap.
        let mut guard = self.access_token.write().unwrap_or_else(|poisoned| poisoned.into_inner());
        *guard = token.into();
    }

    pub(crate) fn url(&self, path: &str) -> String {
        join(&self.base, path)
    }

    pub(crate) fn upload_url(&self, path: &str) -> String {
        join(&self.upload_base, path)
    }

    /// A request with the current bearer attached. Callers building a body of their own
    /// (the resumable upload) start here and hand the result to [`Client::send_raw`].
    pub(crate) fn request(&self, method: Method, url: &str) -> RequestBuilder {
        let token =
            self.access_token.read().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
        self.http.request(method, url).bearer_auth(token)
    }

    pub(crate) async fn get_json<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, &str)],
    ) -> Result<T> {
        let url = append_query(&self.url(path), query);
        into_json(self.execute(|| self.request(Method::GET, &url)).await?).await
    }

    pub(crate) async fn post_json<B: Serialize + Sync, T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, &str)],
        body: &B,
    ) -> Result<T> {
        let url = append_query(&self.url(path), query);
        into_json(self.execute(|| self.request(Method::POST, &url).json(body)).await?).await
    }

    /// For the 204-no-body replies, which have nothing to deserialize.
    pub(crate) async fn patch_json_empty<B: Serialize + Sync>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<()> {
        let response =
            match self.execute(|| self.request(Method::PATCH, &self.url(path)).json(body)).await? {
                Outcome::Failed(err) => return Err(err),
                Outcome::Body(response) => response,
            };
        drain(response).await
    }

    pub(crate) async fn delete(&self, path: &str) -> Result<()> {
        let url = self.url(path);
        match self.execute(|| self.request(Method::DELETE, &url)).await? {
            Outcome::Failed(err) => Err(err),
            Outcome::Body(response) => drain(response).await,
        }
    }

    /// Accepts 200 and 206 alike; the caller needs the status to tell a full body from a
    /// partial one.
    pub(crate) async fn get_bytes(
        &self,
        path: &str,
        range: Option<&str>,
    ) -> Result<(Vec<u8>, StatusCode)> {
        let url = self.url(path);
        let outcome = self
            .execute(|| {
                let request = self.request(Method::GET, &url);
                match range {
                    Some(range) => request.header("Range", range),
                    None => request,
                }
            })
            .await?;

        match outcome {
            Outcome::Failed(err) => Err(err),
            Outcome::Body(response) => {
                let status = response.status();
                let bytes = response.bytes().await.map_err(transport)?;
                Ok((bytes.to_vec(), status))
            }
        }
    }

    /// No retry: the resumable upload owns its own 308 loop and re-sends whole chunks, so
    /// a silent replay here would duplicate bytes.
    pub(crate) async fn send_raw(&self, builder: RequestBuilder) -> Result<Response> {
        builder.send().await.map_err(transport)
    }

    async fn execute(&self, build: impl Fn() -> RequestBuilder) -> Result<Outcome> {
        let mut attempt = 0;
        let mut slept = Duration::ZERO;

        loop {
            attempt += 1;
            let response = build().send().await.map_err(transport)?;
            let status = response.status();
            if status.is_success() {
                return Ok(Outcome::Body(response));
            }

            let retry_after = retry_after_seconds(&response);
            let envelope = parse_envelope(&read_capped(response).await?);
            let rate_limited = retryable(status, envelope.as_ref());
            let failure = map_status(status, retry_after, envelope.as_ref());

            // The hint comes from the server, so the sleep budget gets the final say.
            let wait = match retry_after {
                Some(seconds) => capped_retry_after(seconds),
                None => self.backoff.delay(attempt),
            };
            if !rate_limited || attempt >= MAX_ATTEMPTS || slept + wait > self.backoff.budget {
                return Ok(Outcome::Failed(failure));
            }

            slept += wait;
            sleep(wait).await;
        }
    }
}

async fn into_json<T: DeserializeOwned>(outcome: Outcome) -> Result<T> {
    match outcome {
        Outcome::Failed(err) => Err(err),
        Outcome::Body(response) => {
            let status = response.status();
            let bytes = response.bytes().await.map_err(transport)?;
            // serde's Display carries a line and column, never the payload.
            serde_json::from_slice(&bytes)
                .map_err(|err| ProviderError::Decode(format!("{status}: malformed JSON ({err})")))
        }
    }
}

/// Consumes a success whose body carries nothing, so a 204 does not leave the connection
/// half-read.
async fn drain(response: Response) -> Result<()> {
    response.bytes().await.map(|_| ()).map_err(transport)
}

/// Reads at most [`ERROR_BODY_CAP`], so a 5xx from a proxy cannot buffer a whole video.
pub(crate) async fn read_capped(mut response: Response) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    while out.len() < ERROR_BODY_CAP {
        match response.chunk().await {
            Ok(Some(chunk)) => out.extend_from_slice(&chunk),
            Ok(None) | Err(_) => break,
        }
    }
    out.truncate(ERROR_BODY_CAP);
    Ok(out)
}

#[derive(serde::Deserialize)]
struct DriveEnvelope {
    error: DriveError,
}

#[derive(serde::Deserialize)]
struct DriveError {
    message: Option<String>,
    #[serde(default)]
    errors: Vec<DriveReason>,
}

#[derive(serde::Deserialize)]
struct DriveReason {
    reason: Option<String>,
}

fn parse_envelope(body: &[u8]) -> Option<DriveEnvelope> {
    serde_json::from_slice(body).ok()
}

fn retryable(status: StatusCode, envelope: Option<&DriveEnvelope>) -> bool {
    match status {
        StatusCode::TOO_MANY_REQUESTS => true,
        StatusCode::FORBIDDEN => envelope.is_some_and(|envelope| {
            envelope
                .error
                .errors
                .iter()
                .filter_map(|entry| entry.reason.as_deref())
                .any(|reason| RATE_LIMIT_REASONS.contains(&reason))
        }),
        status if status.is_server_error() => true,
        _ => false,
    }
}

/// Google's reason is more specific than the status, so it leads the message. The body
/// itself never reaches the message: it can be a proxy's HTML page.
fn map_status(
    status: StatusCode,
    retry_after: Option<u64>,
    envelope: Option<&DriveEnvelope>,
) -> ProviderError {
    let reason = envelope
        .and_then(|envelope| envelope.error.errors.first())
        .and_then(|entry| entry.reason.as_deref());
    let message = envelope.and_then(|envelope| envelope.error.message.as_deref());
    let detail = match (reason, message) {
        (Some(reason), Some(message)) => format!("{reason}: {message}"),
        (Some(reason), None) => reason.to_string(),
        (None, Some(message)) => message.to_string(),
        (None, None) => "no error body".to_string(),
    };
    let message = format!("{status}: {detail}");

    match status {
        StatusCode::UNAUTHORIZED => ProviderError::Unauthorized(message),
        StatusCode::FORBIDDEN => ProviderError::Forbidden(message),
        StatusCode::NOT_FOUND => ProviderError::NotFound(message),
        StatusCode::CONFLICT => ProviderError::Conflict(message),
        StatusCode::TOO_MANY_REQUESTS => ProviderError::RateLimited { message, retry_after },
        _ => ProviderError::Provider(message),
    }
}

/// Seconds only: Google sends an integer, and reading an HTTP-date would need a date
/// parser this crate has no reason to carry. The value is passed to the caller verbatim
/// so a UI can schedule the retry; only the sleep here is capped.
fn retry_after_seconds(response: &Response) -> Option<u64> {
    response.headers().get("Retry-After")?.to_str().ok()?.trim().parse().ok()
}

fn capped_retry_after(seconds: u64) -> Duration {
    Duration::from_secs(seconds).min(RETRY_AFTER_CAP)
}

fn transport(err: reqwest::Error) -> ProviderError {
    ProviderError::Transport(err.to_string())
}

fn join(base: &str, path: &str) -> String {
    if path.starts_with("http://") || path.starts_with("https://") {
        return path.to_string();
    }
    format!("{}/{}", base.trim_end_matches('/'), path.trim_start_matches('/'))
}

fn append_query(url: &str, query: &[(&str, &str)]) -> String {
    if query.is_empty() {
        return url.to_string();
    }
    let pairs: Vec<String> = query
        .iter()
        .map(|(key, value)| format!("{}={}", encode_component(key), encode_component(value)))
        .collect();
    format!("{url}?{}", pairs.join("&"))
}

/// RFC 3986 unreserved set, and nothing else. A `q` value carries Drive query literals,
/// so `'` becomes `%27` and a space `%20` (never `+`, which Drive reads literally); a
/// value the caller already escaped as `\'` reaches Drive as `\27` intact. Leaving `&`,
/// `=` or `'` raw would let a filename add query parameters.
fn encode_component(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for byte in raw.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::google::stub::{Reply, Stub};
    use serde::Deserialize;

    const TOKEN: &str = "ya29.test-token";

    #[derive(Debug, Deserialize, PartialEq)]
    struct Probe {
        ok: bool,
    }

    async fn client(stub: &Stub) -> Client {
        Client::with_backoff(
            TOKEN,
            &stub.base,
            &stub.base,
            TEST_DELAY_BASE,
            TEST_DELAY_CAP,
            TEST_SLEEP_BUDGET,
        )
    }

    async fn probe_ok(client: &Client, path: &str) -> Probe {
        client.get_json(path, &[]).await.expect("2xx must decode")
    }

    #[tokio::test]
    async fn default_endpoints_are_the_google_roots() {
        let client = Client::new(TOKEN.into(), DEFAULT_BASE.into(), DEFAULT_UPLOAD_BASE.into());
        assert_eq!(client.url("files"), "https://www.googleapis.com/drive/v3/files");
        assert_eq!(client.upload_url("files"), "https://www.googleapis.com/upload/drive/v3/files");
        assert_eq!(client.url("/files"), "https://www.googleapis.com/drive/v3/files");
    }

    #[tokio::test]
    async fn an_absolute_url_as_path_bypasses_the_base() {
        let stub = Stub::start(vec![Reply::json(200, r#"{"ok":true}"#)]).await;
        let client = client(&stub).await;

        let probe: Probe =
            client.get_json(&format!("{}/files", stub.base), &[]).await.expect("get");
        assert!(probe.ok);
        assert_eq!(stub.requests()[0].path, "/files");
    }

    #[tokio::test]
    async fn every_request_carries_the_bearer_token() {
        let stub = Stub::start(vec![Reply::json(200, r#"{"ok":true}"#)]).await;
        let client = client(&stub).await;

        assert!(probe_ok(&client, "files").await.ok);
        assert_eq!(stub.requests()[0].header("authorization"), Some("Bearer ya29.test-token"));
    }

    #[tokio::test]
    async fn set_access_token_changes_the_next_request_without_a_new_provider() {
        let stub = Stub::start(vec![
            Reply::json(200, r#"{"ok":true}"#),
            Reply::json(200, r#"{"ok":true}"#),
        ])
        .await;
        let client = client(&stub).await;

        probe_ok(&client, "files").await;
        client.set_access_token("ya29.refreshed");
        probe_ok(&client, "about").await;

        let requests = stub.requests();
        assert_eq!(requests[0].header("authorization"), Some("Bearer ya29.test-token"));
        assert_eq!(requests[1].header("authorization"), Some("Bearer ya29.refreshed"));
        assert_eq!(requests[1].path, "/about");
    }

    #[tokio::test]
    async fn debug_never_prints_the_access_token() {
        let rendered = format!(
            "{:?}",
            Client::new(
                "ya29.super-secret".into(),
                DEFAULT_BASE.into(),
                DEFAULT_UPLOAD_BASE.into()
            )
        );
        assert!(!rendered.contains("ya29.super-secret"), "{rendered}");
        assert!(rendered.contains("redacted"), "{rendered}");
    }

    #[tokio::test]
    async fn a_quoted_query_value_reaches_drive_byte_for_byte() {
        let stub = Stub::start(vec![Reply::json(200, r#"{"ok":true}"#)]).await;
        let client = client(&stub).await;
        // A Drive literal holding an apostrophe; the caller escapes the inner quote.
        let query = r#"name = 'a\'b' in parents"#;

        let _: Probe = client.get_json("files", &[("q", query)]).await.expect("get");

        let request = &stub.requests()[0];
        assert_eq!(request.query("q").as_deref(), Some(query));
        assert!(
            request.target.contains("q=name%20%3D%20%27a%5C%27b%27%20in%20parents"),
            "raw target was {}",
            request.target
        );
    }

    #[tokio::test]
    async fn a_value_cannot_inject_a_query_parameter() {
        let stub = Stub::start(vec![Reply::json(200, r#"{"ok":true}"#)]).await;
        let client = client(&stub).await;

        let _: Probe = client
            .get_json("files", &[("q", "name = 'x&trashed = false'"), ("pageSize", "10")])
            .await
            .expect("get");

        let request = &stub.requests()[0];
        assert_eq!(request.query("q").as_deref(), Some("name = 'x&trashed = false'"));
        assert_eq!(request.query("trashed").as_deref(), None);
        assert_eq!(request.query("pageSize").as_deref(), Some("10"));
    }

    #[tokio::test]
    async fn encodes_everything_outside_the_unreserved_set() {
        assert_eq!(encode_component("aZ09-._~"), "aZ09-._~");
        assert_eq!(encode_component("'"), "%27");
        assert_eq!(encode_component("\\'"), "%5C%27");
        assert_eq!(encode_component("a b&c=d"), "a%20b%26c%3Dd");
    }

    #[tokio::test]
    async fn a_429_with_no_retry_hint_is_retried_then_succeeds() {
        let stub = Stub::start(vec![
            Reply::json(429, r#"{"error":{"code":429,"message":"slow down","errors":[{"reason":"rateLimitExceeded"}]}}"#),
            Reply::json(200, r#"{"ok":true}"#),
        ])
        .await;
        let client = client(&stub).await;

        assert!(probe_ok(&client, "files").await.ok);
        assert_eq!(stub.requests().len(), 2);
    }

    #[tokio::test]
    async fn a_per_user_403_is_retried_even_though_it_is_not_a_429() {
        let stub = Stub::start(vec![
            Reply::json(403, r#"{"error":{"code":403,"message":"Rate Limit Exceeded","errors":[{"reason":"userRateLimitExceeded"}]}}"#),
            Reply::json(403, r#"{"error":{"code":403,"message":"Rate Limit Exceeded","errors":[{"reason":"rateLimitExceeded"}]}}"#),
            Reply::json(200, r#"{"ok":true}"#),
        ])
        .await;
        let client = client(&stub).await;

        assert!(probe_ok(&client, "files").await.ok);
        assert_eq!(stub.requests().len(), 3);
    }

    #[tokio::test]
    async fn a_plain_403_is_not_retried() {
        let stub = Stub::start(vec![
            Reply::json(403, r#"{"error":{"code":403,"message":"Insufficient permissions","errors":[{"reason":"insufficientFilePermissions"}]}}"#),
            Reply::json(200, r#"{"ok":true}"#),
        ])
        .await;
        let client = client(&stub).await;

        let err = client.get_json::<Probe>("files", &[]).await.expect_err("must fail");
        assert_eq!(err.code(), "forbidden");
        assert_eq!(stub.requests().len(), 1);
    }

    #[tokio::test]
    async fn a_404_is_called_exactly_once() {
        let stub = Stub::start(vec![
            Reply::json(404, r#"{"error":{"code":404,"message":"File not found","errors":[{"reason":"notFound"}]}}"#),
            Reply::json(200, r#"{"ok":true}"#),
        ])
        .await;
        let client = client(&stub).await;

        let err = client.get_json::<Probe>("files/nope", &[]).await.expect_err("must fail");
        assert_eq!(err.code(), "not_found");
        assert_eq!(stub.requests().len(), 1);
    }

    #[tokio::test]
    async fn a_5xx_is_retried_up_to_the_attempt_cap() {
        let stub = Stub::start(vec![Reply::new(500, "boom")]).await;
        let client = client(&stub).await;

        let err = client.get_json::<Probe>("files", &[]).await.expect_err("must fail");
        assert_eq!(err.code(), "provider");
        assert_eq!(stub.requests().len(), MAX_ATTEMPTS as usize);
    }

    #[tokio::test]
    async fn retry_after_seconds_reach_the_rate_limited_error() {
        let stub =
            Stub::start(vec![Reply::new(429, "slow down").header("Retry-After", "30")]).await;
        let client = client(&stub).await;

        let err = client.get_json::<Probe>("files", &[]).await.expect_err("must fail");
        assert_eq!(err.code(), "rate_limited");
        assert_eq!(err.retry_after(), Some(30));
        assert!(err.to_string().contains("30"), "{err}");
        // The hint exceeds the sleep budget, so it is reported instead of obeyed.
        assert_eq!(stub.requests().len(), 1);
    }

    #[tokio::test]
    async fn a_zero_retry_after_is_obeyed_and_the_call_succeeds() {
        let stub = Stub::start(vec![
            Reply::new(429, "slow down").header("Retry-After", "0"),
            Reply::json(200, r#"{"ok":true}"#),
        ])
        .await;
        let client = client(&stub).await;

        assert!(probe_ok(&client, "files").await.ok);
        assert_eq!(stub.requests().len(), 2);
    }

    #[tokio::test]
    async fn a_retry_after_too_long_to_wait_is_reported_rather_than_obeyed() {
        let stub =
            Stub::start(vec![Reply::new(429, "slow down").header("Retry-After", "86400")]).await;
        let client = client(&stub).await;

        let err = client.get_json::<Probe>("files", &[]).await.expect_err("must fail");
        // The server's own value survives into the error; the sleep is what gets capped.
        assert_eq!(err.retry_after(), Some(86_400));
        assert_eq!(stub.requests().len(), 1);
    }

    #[test]
    fn the_slept_retry_after_is_capped_at_thirty_seconds() {
        assert_eq!(capped_retry_after(0), Duration::ZERO);
        assert_eq!(capped_retry_after(30), Duration::from_secs(30));
        assert_eq!(capped_retry_after(86_400), Duration::from_secs(30));
        assert_eq!(capped_retry_after(u64::MAX), Duration::from_secs(30));
    }

    #[tokio::test]
    async fn the_error_body_is_capped_not_buffered_whole() {
        let junk = "j".repeat(200 * 1024);
        let stub = Stub::start(vec![Reply::new(200, &junk)]).await;
        let response = reqwest::get(&stub.base).await.expect("get");

        assert_eq!(read_capped(response).await.expect("capped read").len(), ERROR_BODY_CAP);
    }

    #[tokio::test]
    async fn a_5xx_with_a_huge_body_maps_to_an_error_without_reading_all_of_it() {
        let junk = "j".repeat(10 * 1024 * 1024);
        let stub = Stub::start(vec![Reply::new(500, &junk)]).await;
        let client = client(&stub).await;

        let err = client.get_json::<Probe>("files", &[]).await.expect_err("must fail");
        assert_eq!(err.code(), "provider");
        assert!(err.to_string().len() < 200, "message was {}", err.to_string().len());
    }

    #[tokio::test]
    async fn the_google_envelope_maps_to_the_right_variant() {
        let cases = [
            (401u16, "authError", "unauthorized"),
            (403, "insufficientPermissions", "forbidden"),
            (404, "notFound", "not_found"),
            (409, "conflict", "conflict"),
            (400, "badRequest", "provider"),
        ];

        for (status, reason, expected) in cases {
            let body = format!(
                r#"{{"error":{{"code":{status},"message":"Google says no","errors":[{{"reason":"{reason}"}}]}}}}"#
            );
            let stub = Stub::start(vec![Reply::new(status, &body)]).await;
            let client = client(&stub).await;

            let err = client.get_json::<Probe>("files", &[]).await.expect_err("must fail");
            assert_eq!(err.code(), expected, "status {status}");
            let rendered = err.to_string();
            // Google's reason beats the bare status, and its message is kept.
            assert!(rendered.contains(&status.to_string()), "{rendered}");
            assert!(rendered.contains(reason), "{rendered}");
            assert!(rendered.contains("Google says no"), "{rendered}");
        }
    }

    #[tokio::test]
    async fn an_error_message_never_carries_the_raw_body() {
        let secret = "ya29.token-echoed-by-a-proxy";
        let stub = Stub::start(vec![Reply::new(
            403,
            &format!(r#"{{"error":{{"message":"denied {secret}"}}}}"#),
        )])
        .await;
        let client = client(&stub).await;

        let err = client.get_json::<Probe>("files", &[]).await.expect_err("must fail");
        let rendered = err.to_string();
        // Google's own message is kept; the surrounding body is not.
        assert!(rendered.contains("denied"), "{rendered}");
        assert!(!rendered.contains('{') && !rendered.contains('}'), "{rendered}");
    }

    #[tokio::test]
    async fn an_unparsable_error_body_yields_the_status_alone() {
        let stub = Stub::start(vec![Reply::new(503, "<html>gateway down</html>")]).await;
        let client = client(&stub).await;

        let err = client.get_json::<Probe>("files", &[]).await.expect_err("must fail");
        assert_eq!(err.code(), "provider");
        assert!(err.to_string().contains("503"), "{err}");
        assert!(!err.to_string().contains("gateway down"), "{err}");
    }

    #[tokio::test]
    async fn a_malformed_success_body_is_a_decode_error() {
        let stub = Stub::start(vec![Reply::new(200, "{not json")]).await;
        let client = client(&stub).await;

        let err = client.get_json::<Probe>("files", &[]).await.expect_err("must fail");
        assert_eq!(err.code(), "decode");
    }

    #[tokio::test]
    async fn a_204_answers_an_empty_patch_and_a_delete_cleanly() {
        let stub = Stub::start(vec![Reply::empty(204), Reply::empty(204)]).await;
        let client = client(&stub).await;

        client
            .patch_json_empty("files/1", &serde_json::json!({"trashed": true}))
            .await
            .expect("patch");
        client.delete("files/1").await.expect("delete");

        let requests = stub.requests();
        assert_eq!(requests[0].method, "PATCH");
        assert_eq!(requests[0].path, "/files/1");
        assert!(requests[0].body.contains("\"trashed\":true"), "{}", requests[0].body);
        assert_eq!(requests[1].method, "DELETE");
    }

    #[tokio::test]
    async fn a_post_sends_its_json_body_alongside_the_query() {
        let stub = Stub::start(vec![Reply::json(200, r#"{"id":"folder-1"}"#)]).await;
        let client = client(&stub).await;

        let created: serde_json::Value = client
            .post_json(
                "files",
                &[("fields", "id"), ("supportsAllDrives", "true")],
                &serde_json::json!({"name": "9drive"}),
            )
            .await
            .expect("post");

        assert_eq!(created["id"], "folder-1");
        let request = &stub.requests()[0];
        assert_eq!(request.method, "POST");
        assert_eq!(request.query("fields").as_deref(), Some("id"));
        assert_eq!(request.query("supportsAllDrives").as_deref(), Some("true"));
        assert!(request.body.contains(r#""name":"9drive""#), "{}", request.body);
        assert!(request.header("authorization").is_some());
    }

    #[tokio::test]
    async fn get_bytes_accepts_both_200_and_206() {
        let stub = Stub::start(vec![
            Reply::new(200, "whole file"),
            Reply::new(206, "first 4").header("Content-Range", "bytes 0-4/9"),
        ])
        .await;
        let client = client(&stub).await;

        let (body, status) = client.get_bytes("files/1", None).await.expect("full");
        assert_eq!(body, b"whole file");
        assert_eq!(status, StatusCode::OK);

        let (body, status) = client.get_bytes("files/1", Some("bytes=0-4")).await.expect("partial");
        assert_eq!(body, b"first 4");
        assert_eq!(status, StatusCode::PARTIAL_CONTENT);
        assert_eq!(stub.requests()[1].header("range"), Some("bytes=0-4"));
    }

    #[tokio::test]
    async fn send_raw_returns_the_response_without_retrying() {
        let stub = Stub::start(vec![Reply::new(308, "resume?upload_id=1")]).await;
        let client = client(&stub).await;

        let url = client.upload_url("files?uploadType=resumable");
        let response = client.send_raw(client.request(Method::POST, &url)).await.expect("send");

        assert_eq!(response.status(), StatusCode::PERMANENT_REDIRECT);
        assert_eq!(stub.requests().len(), 1);
        assert_eq!(stub.requests()[0].header("authorization"), Some("Bearer ya29.test-token"));
    }

    #[tokio::test]
    async fn a_transport_failure_is_not_retried() {
        let client = Client::with_backoff(
            TOKEN,
            "http://127.0.0.1:1",
            "http://127.0.0.1:1",
            TEST_DELAY_BASE,
            TEST_DELAY_CAP,
            TEST_SLEEP_BUDGET,
        );

        let err = client.get_json::<Probe>("files", &[]).await.expect_err("must fail");
        assert_eq!(err.code(), "transport");
    }
}
