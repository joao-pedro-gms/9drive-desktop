//! Access-token refresh, with `invalid_grant` reported as "reconnect" (issue #26).

use crate::endpoints::Endpoints;
use crate::error::Result;
use crate::token::{TokenSet, request_token};

/// Trades a refresh token for a fresh access token. The returned [`TokenSet`] carries
/// no refresh token: Google only sends one when consent is (re)granted, so the caller
/// keeps the one it already has in the keychain.
pub async fn refresh_access_token(
    http: &reqwest::Client,
    endpoints: &Endpoints,
    client_id: &str,
    refresh_token: &str,
) -> Result<TokenSet> {
    request_token(
        http,
        &endpoints.token,
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", client_id),
        ],
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stub::Stub;

    #[tokio::test]
    async fn posts_the_refresh_grant() {
        let stub = Stub::start(200, r#"{"access_token":"at-2","expires_in":3599}"#).await;

        let tokens = refresh_access_token(
            &reqwest::Client::new(),
            &Endpoints { token: stub.url.clone(), ..Endpoints::default() },
            "id.apps.googleusercontent.com",
            "1//refresh-token",
        )
        .await
        .expect("refresh");

        assert_eq!(tokens.access_token, "at-2");
        assert_eq!(tokens.refresh_token, None, "Google omits it on refresh");

        let request = stub.request().await;
        assert_eq!(request.field("grant_type").as_deref(), Some("refresh_token"));
        assert_eq!(request.field("refresh_token").as_deref(), Some("1//refresh-token"));
        assert_eq!(request.field("client_id").as_deref(), Some("id.apps.googleusercontent.com"));
    }

    #[tokio::test]
    async fn an_invalid_grant_asks_for_reauthentication() {
        let stub = Stub::start(
            400,
            r#"{"error":"invalid_grant","error_description":"Token has been expired or revoked."}"#,
        )
        .await;

        let err = refresh_access_token(
            &reqwest::Client::new(),
            &Endpoints { token: stub.url.clone(), ..Endpoints::default() },
            "id",
            "1//revoked",
        )
        .await
        .expect_err("must fail");

        assert_eq!(err.code(), "reauthenticate");
        assert!(err.to_string().contains("expired or revoked"));
        stub.request().await;
    }

    #[tokio::test]
    async fn debug_never_prints_the_refreshed_token() {
        let tokens = TokenSet {
            access_token: "ya29.leaked".into(),
            refresh_token: None,
            expires_in: Some(3599),
            scope: None,
        };
        assert!(!format!("{tokens:?}").contains("ya29.leaked"));
    }
}
