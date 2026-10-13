//! The two Drive calls the connect and disconnect flows need (issues #27 and #28).

use serde::Deserialize;

use crate::endpoints::Endpoints;
use crate::error::{OAuthError, Result};

/// What `about?fields=user` gives for the account card.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AccountInfo {
    #[serde(rename = "displayName")]
    pub name: Option<String>,
    #[serde(rename = "emailAddress")]
    pub email: String,
}

#[derive(Deserialize)]
struct AboutResponse {
    user: AccountInfo,
}

/// `about.get` with `fields=user`, so no scope beyond `drive` is needed.
pub async fn fetch_account_info(
    http: &reqwest::Client,
    endpoints: &Endpoints,
    access_token: &str,
) -> Result<AccountInfo> {
    let response = http
        .get(format!("{}/about?fields=user", endpoints.drive))
        .bearer_auth(access_token)
        .send()
        .await?;
    let status = response.status();
    let body = response.bytes().await?;

    if !status.is_success() {
        return Err(OAuthError::Protocol(format!("about.get returned {status}")));
    }

    serde_json::from_slice::<AboutResponse>(&body)
        .map(|parsed| parsed.user)
        .map_err(|e| OAuthError::Protocol(format!("about.get user: {e}")))
}

/// Revokes a token so an explicit disconnect does not leave a live grant behind.
pub async fn revoke(http: &reqwest::Client, endpoints: &Endpoints, token: &str) -> Result<()> {
    let response = http.post(&endpoints.revoke).form(&[("token", token)]).send().await?;
    let status = response.status();
    if status.is_success() {
        return Ok(());
    }
    Err(OAuthError::Protocol(format!("revoke returned {status}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stub::Stub;

    #[tokio::test]
    async fn about_user_gives_the_name_and_email() {
        let stub = Stub::start(
            200,
            r#"{"user":{"displayName":"Test Person","emailAddress":"p@example.invalid"}}"#,
        )
        .await;

        let account = fetch_account_info(
            &reqwest::Client::new(),
            &Endpoints { drive: stub.url.clone(), ..Endpoints::default() },
            "ya29.at-test",
        )
        .await
        .expect("about");

        assert_eq!(account.name.as_deref(), Some("Test Person"));
        assert_eq!(account.email, "p@example.invalid");

        let request = stub.request().await;
        assert_eq!(request.method, "GET");
        assert_eq!(request.target, "/about?fields=user");
        assert_eq!(request.header("authorization"), Some("Bearer ya29.at-test"));
    }

    #[tokio::test]
    async fn a_missing_user_payload_is_a_protocol_error() {
        let stub = Stub::start(200, r#"{"storageQuota":{}}"#).await;
        let err = fetch_account_info(
            &reqwest::Client::new(),
            &Endpoints { drive: stub.url.clone(), ..Endpoints::default() },
            "ya29.at-test",
        )
        .await
        .expect_err("must fail");
        assert_eq!(err.code(), "protocol");
        stub.request().await;
    }

    #[tokio::test]
    async fn revoke_posts_the_token() {
        let stub = Stub::start(200, "").await;
        revoke(
            &reqwest::Client::new(),
            &Endpoints { revoke: stub.url.clone(), ..Endpoints::default() },
            "ya29.to-revoke",
        )
        .await
        .expect("revoke");

        let request = stub.request().await;
        assert_eq!(request.method, "POST");
        assert_eq!(request.field("token").as_deref(), Some("ya29.to-revoke"));
    }

    #[tokio::test]
    async fn a_failed_revoke_is_reported_so_the_caller_still_clears_the_keychain() {
        let stub = Stub::start(500, "boom").await;
        let err = revoke(
            &reqwest::Client::new(),
            &Endpoints { revoke: stub.url.clone(), ..Endpoints::default() },
            "ya29.to-revoke",
        )
        .await
        .expect_err("must fail");
        assert_eq!(err.code(), "protocol");
        assert!(err.to_string().contains("500"));
        stub.request().await;
    }
}
