//! The whole connect flow, so `src-tauri` only has to open a browser.

use crate::client_id::ClientId;
use crate::endpoints::{DRIVE_SCOPE, Endpoints};
use crate::error::Result;
use crate::google::{self, AccountInfo};
use crate::listener::Loopback;
use crate::pkce::{CodeVerifier, State};
use crate::token::{self, TokenSet};

/// A completed authorization. The refresh token is deliberately not persisted here:
/// the caller stores it in the keychain, which keeps this testable on machines with
/// no credential store.
#[derive(Debug)]
pub struct Authorization {
    pub tokens: TokenSet,
    pub account: AccountInfo,
}

/// Binds the loopback listener, opens the consent screen, waits for the redirect,
/// exchanges the code and reads the account profile.
///
/// `open_browser` is the only platform-specific piece; `src-tauri` passes something
/// that hands the URL to the system browser. If it cannot, the flow ends as
/// [`OAuthError::Cancelled`](crate::error::OAuthError::Cancelled): no grant exists
/// to continue with.
pub async fn authorize(
    http: &reqwest::Client,
    endpoints: &Endpoints,
    client_id: &ClientId,
    open_browser: impl FnOnce(&str) -> Result<()>,
) -> Result<Authorization> {
    let loopback = Loopback::bind().await?;
    let redirect_uri = loopback.redirect_uri();
    let verifier = CodeVerifier::generate()?;
    let state = State::generate()?;

    let url = token::authorize_url(
        endpoints,
        client_id.as_str(),
        &redirect_uri,
        DRIVE_SCOPE,
        &verifier,
        &state,
    );
    open_browser(&url)?;

    let code = loopback.wait_for_code(&state).await?;
    let tokens =
        token::exchange_code(http, endpoints, client_id.as_str(), &redirect_uri, &code, &verifier)
            .await?;
    let account = google::fetch_account_info(http, endpoints, &tokens.access_token).await?;

    Ok(Authorization { tokens, account })
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    use super::*;
    use crate::client_id::resolve_client_id;
    use crate::listener::decode_component;
    use crate::stub::Stub;

    /// Reads one parameter out of the consent URL, undoing the query encoding.
    fn consent_field(url: &str, name: &str) -> String {
        let query = url.split_once('?').expect("query").1;
        query
            .split('&')
            .filter_map(|pair| pair.split_once('='))
            .find(|(key, _)| *key == name)
            .map(|(_, value)| decode_component(value))
            .unwrap_or_else(|| panic!("{name} missing from {url}"))
    }

    /// Plays the browser: takes the consent URL and calls the loopback redirect.
    fn play_browser(consent_url: &str, target: &str) {
        let host =
            consent_field(consent_url, "redirect_uri").trim_start_matches("http://").to_string();
        let target = format!("/?{target}");

        tokio::spawn(async move {
            let mut stream = TcpStream::connect(&host).await.expect("connect");
            let request = format!("GET {target} HTTP/1.1\r\nHost: {host}\r\n\r\n");
            stream.write_all(request.as_bytes()).await.expect("write");
            let mut response = String::new();
            stream.read_to_string(&mut response).await.expect("read");
            assert!(response.contains("close this tab"), "{response}");
        });
    }

    fn test_client_id() -> ClientId {
        resolve_client_id(Some("flow-test.apps.googleusercontent.com")).expect("id")
    }

    #[tokio::test]
    async fn connects_an_account_end_to_end() {
        let token_stub = Stub::start(
            200,
            r#"{"access_token":"at-flow","refresh_token":"rt-flow","expires_in":3599,"scope":"drive"}"#,
        )
        .await;
        let drive_stub = Stub::start(
            200,
            r#"{"user":{"displayName":"Flow Tester","emailAddress":"flow@example.invalid"}}"#,
        )
        .await;

        let endpoints = Endpoints {
            token: token_stub.url.clone(),
            drive: drive_stub.url.clone(),
            authorize: "https://accounts.google.com/o/oauth2/v2/auth".into(),
            ..Endpoints::default()
        };
        let http = reqwest::Client::new();

        let state = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let captured = std::sync::Arc::clone(&state);
        let authorization = authorize(&http, &endpoints, &test_client_id(), |url| {
            *captured.lock().expect("lock") = url.to_string();
            play_browser(url, &format!("code=4%2F0abc&state={}", consent_field(url, "state")));
            Ok(())
        })
        .await
        .expect("authorize");

        assert_eq!(authorization.account.name.as_deref(), Some("Flow Tester"));
        assert_eq!(authorization.account.email, "flow@example.invalid");
        assert_eq!(authorization.tokens.access_token, "at-flow");
        assert_eq!(authorization.tokens.refresh_token.as_deref(), Some("rt-flow"));

        let consent_url = state.lock().expect("lock").clone();
        assert!(consent_url.starts_with("https://accounts.google.com/o/oauth2/v2/auth?"));

        let token_request = token_stub.request().await;
        assert_eq!(token_request.field("grant_type").as_deref(), Some("authorization_code"));
        assert_eq!(token_request.field("code").as_deref(), Some("4/0abc"));
        assert_eq!(
            token_request.field("client_id").as_deref(),
            Some("flow-test.apps.googleusercontent.com")
        );
        let redirect = token_request.field("redirect_uri").expect("redirect_uri");
        assert_eq!(redirect, consent_field(&consent_url, "redirect_uri"));
        assert!(
            redirect.starts_with("http://127.0.0.1:") && !redirect.ends_with(":0"),
            "{redirect}"
        );
        assert_eq!(
            token_request.field("code_verifier").map(|value| value.len()),
            Some(43),
            "the PKCE verifier must be sent"
        );

        let drive_request = drive_stub.request().await;
        assert_eq!(drive_request.target, "/about?fields=user");
        assert_eq!(drive_request.header("authorization"), Some("Bearer at-flow"));
    }

    #[tokio::test]
    async fn a_denied_consent_screen_stops_before_the_token_endpoint() {
        // The token endpoint points at a closed port: reaching it would surface an
        // `http` error instead of `cancelled`.
        let endpoints = Endpoints { token: "http://127.0.0.1:1".into(), ..Endpoints::default() };

        let err = authorize(&reqwest::Client::new(), &endpoints, &test_client_id(), |url| {
            play_browser(url, "error=access_denied");
            Ok(())
        })
        .await
        .expect_err("must stop at the consent screen");

        assert_eq!(err.code(), "cancelled");
        assert!(err.to_string().contains("access_denied"));
    }
}
