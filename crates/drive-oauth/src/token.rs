//! Authorize URL construction and the code → token exchange.

use std::fmt;

use serde::Deserialize;

use crate::endpoints::Endpoints;
use crate::error::{OAuthError, Result};
use crate::pkce::{CodeVerifier, State};

/// What the token endpoint returns, minus the refresh token when Google omits it
/// (which it does on refresh).
#[derive(Clone, PartialEq, Eq)]
pub struct TokenSet {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_in: Option<u64>,
    pub scope: Option<String>,
}

// Tokens must never reach a log line, so Debug is written by hand.
impl fmt::Debug for TokenSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TokenSet")
            .field("access_token", &"<redacted>")
            .field("refresh_token", &self.refresh_token.as_ref().map(|_| "<redacted>"))
            .field("expires_in", &self.expires_in)
            .field("scope", &self.scope)
            .finish()
    }
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: Option<u64>,
    scope: Option<String>,
}

#[derive(Deserialize)]
struct TokenErrorBody {
    error: String,
    error_description: Option<String>,
}

/// The URL to open in the browser. `prompt=consent` makes Google return a refresh
/// token even for an account that already granted the scope.
pub fn authorize_url(
    endpoints: &Endpoints,
    client_id: &str,
    redirect_uri: &str,
    scope: &str,
    verifier: &CodeVerifier,
    state: &State,
) -> String {
    let challenge = verifier.challenge();
    let params = [
        ("client_id", client_id),
        ("redirect_uri", redirect_uri),
        ("response_type", "code"),
        ("scope", scope),
        ("code_challenge", challenge.as_str()),
        ("code_challenge_method", "S256"),
        ("state", state.state()),
        ("access_type", "offline"),
        ("prompt", "consent"),
    ];
    let query = params
        .iter()
        .map(|(key, value)| format!("{key}={}", encode_component(value)))
        .collect::<Vec<_>>()
        .join("&");
    format!("{}?{query}", endpoints.authorize)
}

/// Exchanges an authorization code. No `client_secret`: it is not applicable to
/// installed applications (D6, issue #24).
pub async fn exchange_code(
    http: &reqwest::Client,
    endpoints: &Endpoints,
    client_id: &str,
    redirect_uri: &str,
    code: &str,
    verifier: &CodeVerifier,
) -> Result<TokenSet> {
    request_token(
        http,
        &endpoints.token,
        &[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("code_verifier", verifier.verifier()),
            ("redirect_uri", redirect_uri),
            ("client_id", client_id),
        ],
    )
    .await
}

/// Shared by the code exchange and the refresh, so both report Google's error codes
/// the same way.
pub(crate) async fn request_token(
    http: &reqwest::Client,
    url: &str,
    form: &[(&str, &str)],
) -> Result<TokenSet> {
    let response = http.post(url).form(&form).send().await?;
    let status = response.status();
    let body = response.bytes().await?;

    if !status.is_success() {
        return Err(token_error(status, &body));
    }

    // Parsed by hand rather than through `Response::json` so a malformed body is a
    // Protocol error and so the payload never lands inside a reqwest error message.
    let parsed: TokenResponse = serde_json::from_slice(&body)
        .map_err(|e| OAuthError::Protocol(format!("token response: {e}")))?;
    Ok(TokenSet {
        access_token: parsed.access_token,
        refresh_token: parsed.refresh_token,
        expires_in: parsed.expires_in,
        scope: parsed.scope,
    })
}

/// Google's `invalid_grant` is terminal: the refresh token was revoked or the account
/// changed, so the caller has to reconnect rather than retry.
fn token_error(status: reqwest::StatusCode, body: &[u8]) -> OAuthError {
    let parsed = serde_json::from_slice::<TokenErrorBody>(body).ok();
    let reason = parsed
        .as_ref()
        .map(|error| error.error_description.clone().unwrap_or_else(|| error.error.clone()))
        .unwrap_or_else(|| "no error body".into());

    if parsed.as_ref().is_some_and(|error| error.error == "invalid_grant") {
        OAuthError::Reauthenticate(reason)
    } else {
        OAuthError::Protocol(format!("token endpoint returned {status}: {reason}"))
    }
}

/// RFC 3986 unreserved set. `client_id` comes from user input, so it cannot be
/// concatenated into a query without escaping.
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
    use crate::endpoints::DRIVE_SCOPE;
    use crate::stub::Stub;

    const TOKEN_JSON: &str = r#"{"access_token":"at-test","refresh_token":"rt-test","expires_in":3599,"scope":"https://www.googleapis.com/auth/drive"}"#;

    #[tokio::test]
    async fn authorize_url_carries_every_required_parameter() {
        let verifier = CodeVerifier::from_raw("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk");
        let state = State::from_raw("state-value");
        let url = authorize_url(
            &Endpoints::default(),
            "id.apps.googleusercontent.com",
            "http://127.0.0.1:54321",
            DRIVE_SCOPE,
            &verifier,
            &state,
        );

        assert!(url.starts_with("https://accounts.google.com/o/oauth2/v2/auth?"));
        for expected in [
            "client_id=id.apps.googleusercontent.com",
            "response_type=code",
            "code_challenge=E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM",
            "code_challenge_method=S256",
            "state=state-value",
            "access_type=offline",
            "prompt=consent",
            "redirect_uri=http%3A%2F%2F127.0.0.1%3A54321",
            "scope=https%3A%2F%2Fwww.googleapis.com%2Fauth%2Fdrive",
        ] {
            assert!(url.contains(expected), "{expected} missing from {url}");
        }
    }

    #[tokio::test]
    async fn a_client_id_cannot_inject_extra_parameters() {
        let url = authorize_url(
            &Endpoints::default(),
            "evil&access_type=online",
            "http://127.0.0.1:1",
            DRIVE_SCOPE,
            &CodeVerifier::from_raw("v"),
            &State::from_raw("s"),
        );
        assert!(url.contains("client_id=evil%26access_type%3Donline"));
        assert_eq!(
            url.matches("access_type=").count(),
            1,
            "the injected pair must be escaped: {url}"
        );
        assert!(url.contains("access_type=offline"));
    }

    #[tokio::test]
    async fn exchange_code_posts_the_verifier_and_returns_the_refresh_token() {
        let stub = Stub::start(200, TOKEN_JSON).await;
        let http = reqwest::Client::new();
        let verifier = CodeVerifier::generate().expect("verifier");

        let tokens = exchange_code(
            &http,
            &Endpoints { token: stub.url.clone(), ..Endpoints::default() },
            "id.apps.googleusercontent.com",
            "http://127.0.0.1:54321",
            "4/0abc",
            &verifier,
        )
        .await
        .expect("exchange");

        assert_eq!(tokens.access_token, "at-test");
        assert_eq!(tokens.refresh_token.as_deref(), Some("rt-test"));
        assert_eq!(tokens.expires_in, Some(3599));

        let request = stub.request().await;
        assert_eq!(request.method, "POST");
        assert_eq!(request.field("grant_type").as_deref(), Some("authorization_code"));
        assert_eq!(request.field("code").as_deref(), Some("4/0abc"));
        assert_eq!(request.field("code_verifier").as_deref(), Some(verifier.verifier()));
        assert_eq!(request.field("redirect_uri").as_deref(), Some("http://127.0.0.1:54321"));
        assert_eq!(request.field("client_id").as_deref(), Some("id.apps.googleusercontent.com"));
        // No client_secret is ever sent: it is not applicable to installed apps.
        assert!(request.field("client_secret").is_none());
    }

    #[tokio::test]
    async fn malformed_json_is_a_protocol_error() {
        let stub = Stub::start(200, "{not json").await;
        let err = exchange_code(
            &reqwest::Client::new(),
            &Endpoints { token: stub.url.clone(), ..Endpoints::default() },
            "id",
            "http://127.0.0.1:1",
            "code",
            &CodeVerifier::from_raw("v"),
        )
        .await
        .expect_err("must fail");
        assert_eq!(err.code(), "protocol");
        stub.request().await;
    }

    #[tokio::test]
    async fn a_response_without_an_access_token_is_a_protocol_error() {
        let stub = Stub::start(200, r#"{"token_type":"Bearer"}"#).await;
        let err = exchange_code(
            &reqwest::Client::new(),
            &Endpoints { token: stub.url.clone(), ..Endpoints::default() },
            "id",
            "http://127.0.0.1:1",
            "code",
            &CodeVerifier::from_raw("v"),
        )
        .await
        .expect_err("must fail");
        assert_eq!(err.code(), "protocol");
        assert!(err.to_string().contains("access_token"));
        stub.request().await;
    }

    #[tokio::test]
    async fn a_rejected_grant_maps_to_reauthenticate() {
        let stub =
            Stub::start(400, r#"{"error":"invalid_grant","error_description":"Bad Request"}"#)
                .await;
        let err = exchange_code(
            &reqwest::Client::new(),
            &Endpoints { token: stub.url.clone(), ..Endpoints::default() },
            "id",
            "http://127.0.0.1:1",
            "code",
            &CodeVerifier::from_raw("v"),
        )
        .await
        .expect_err("must fail");
        assert_eq!(err.code(), "reauthenticate");
        stub.request().await;
    }

    #[tokio::test]
    async fn other_errors_report_the_status_and_reason_without_the_body() {
        let stub =
            Stub::start(403, r#"{"error":"access_denied","error_description":"nope"}"#).await;
        let err = exchange_code(
            &reqwest::Client::new(),
            &Endpoints { token: stub.url.clone(), ..Endpoints::default() },
            "id",
            "http://127.0.0.1:1",
            "code",
            &CodeVerifier::from_raw("v"),
        )
        .await
        .expect_err("must fail");
        assert_eq!(err.code(), "protocol");
        assert!(err.to_string().contains("403") && err.to_string().contains("nope"));
        stub.request().await;
    }

    #[test]
    fn debug_never_prints_a_token() {
        let tokens = TokenSet {
            access_token: "ya29.super-secret".into(),
            refresh_token: Some("1//refresh-secret".into()),
            expires_in: Some(3599),
            scope: Some(DRIVE_SCOPE.into()),
        };
        let rendered = format!("{tokens:?}");
        assert!(!rendered.contains("ya29.super-secret"));
        assert!(!rendered.contains("1//refresh-secret"));
        assert!(rendered.contains("redacted"));
    }

    #[test]
    fn encodes_everything_outside_the_unreserved_set() {
        assert_eq!(encode_component("aZ09-._~"), "aZ09-._~");
        assert_eq!(encode_component("a b&c"), "a%20b%26c");
        assert_eq!(encode_component("https://x/y"), "https%3A%2F%2Fx%2Fy");
    }

    #[test]
    fn the_endpoints_default_to_google() {
        let endpoints = Endpoints::default();
        assert_eq!(endpoints.authorize, "https://accounts.google.com/o/oauth2/v2/auth");
        assert_eq!(endpoints.token, "https://oauth2.googleapis.com/token");
        assert_eq!(endpoints.revoke, "https://oauth2.googleapis.com/revoke");
        assert_eq!(endpoints.drive, "https://www.googleapis.com/drive/v3");
    }
}
