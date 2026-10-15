//! One-shot loopback listener for Google's redirect: the desktop-app flow (D6).

use std::net::Ipv4Addr;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::error::{OAuthError, Result};
use crate::pkce::State;

/// A browser that never comes back must not hold the app open forever.
const WAIT_TIMEOUT: Duration = Duration::from_secs(30);

/// The redirect is a bare GET; anything larger is not the browser we are waiting for.
const MAX_REQUEST: usize = 8 * 1024;

const CLOSED_PAGE: &str = "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
<title>9Drive Desktop</title><style>body{font-family:system-ui,sans-serif;margin:0;display:flex;\
height:100vh;align-items:center;justify-content:center;background:#0f172a;color:#e2e8f0}\
main{text-align:center}h1{font-size:1.5rem}p{color:#94a3b8}</style></head><body><main>\
<h1>You can close this tab</h1><p>9Drive Desktop has the code. You can go back to the app.</p>\
</main></body></html>";

/// A bound `127.0.0.1` socket waiting for exactly one redirect.
pub struct Loopback {
    listener: TcpListener,
    port: u16,
    timeout: Duration,
}

impl Loopback {
    pub async fn bind() -> Result<Self> {
        Self::bind_with_timeout(WAIT_TIMEOUT).await
    }

    /// The give-up deadline is a parameter so the abandon path is testable without
    /// spending 30 seconds on it.
    pub(crate) async fn bind_with_timeout(timeout: Duration) -> Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .map_err(|e| OAuthError::Listener(format!("bind 127.0.0.1:0: {e}")))?;
        let port = listener
            .local_addr()
            .map_err(|e| OAuthError::Listener(format!("local_addr: {e}")))?
            .port();
        Ok(Self { listener, port, timeout })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// The `redirect_uri` that must be sent both to Google and to the token endpoint.
    pub fn redirect_uri(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// Answers one redirect and drops the socket, so the port is free again.
    pub async fn wait_for_code(self, expected: &State) -> Result<String> {
        let Self { listener, timeout, .. } = self;

        let exchange = async {
            let (mut stream, _) = listener
                .accept()
                .await
                .map_err(|e| OAuthError::Listener(format!("accept: {e}")))?;
            let head = read_head(&mut stream).await?;
            let outcome = parse_callback(&head, expected);
            let _ = stream.write_all(&reply()).await;
            outcome
        };

        match tokio::time::timeout(timeout, exchange).await {
            Ok(result) => result,
            Err(_) => Err(OAuthError::Cancelled(
                "the browser never returned to the loopback listener".into(),
            )),
        }
    }
}

async fn read_head(stream: &mut TcpStream) -> Result<String> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        let read = stream
            .read(&mut chunk)
            .await
            .map_err(|e| OAuthError::Protocol(format!("read callback: {e}")))?;
        if read == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..read]);
        if buf.windows(4).any(|window| window == b"\r\n\r\n") || buf.len() >= MAX_REQUEST {
            break;
        }
    }
    String::from_utf8(buf).map_err(|_| OAuthError::Protocol("callback was not valid UTF-8".into()))
}

/// Validates the callback and returns the authorization code.
fn parse_callback(head: &str, expected: &State) -> Result<String> {
    let target = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .ok_or_else(|| OAuthError::Protocol("callback had no request target".into()))?;

    let query = target.split_once('?').map(|(_, query)| query).unwrap_or("");
    let params: Vec<(String, String)> = query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .filter_map(|pair| {
            let (key, value) = pair.split_once('=')?;
            Some((decode_component(key), decode_component(value)))
        })
        .collect();
    let field =
        |name: &str| params.iter().find(|(key, _)| key == name).map(|(_, value)| value.clone());

    // A denied consent screen comes back with ?error= and no code.
    if let Some(error) = field("error") {
        return Err(OAuthError::Cancelled(format!("Google returned error={error}")));
    }
    let state =
        field("state").ok_or_else(|| OAuthError::Protocol("callback carried no state".into()))?;
    if state != expected.state() {
        return Err(OAuthError::Protocol("callback state did not match the request".into()));
    }
    field("code").ok_or_else(|| OAuthError::Protocol("callback carried no code".into()))
}

/// Reverse of the encoding the authorize URL needs, applied to Google's query values.
pub(crate) fn decode_component(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let nibble = |byte: u8| (byte as char).to_digit(16).map(|digit| digit as u8);
        match bytes[index] {
            b'%' if index + 2 < bytes.len() => {
                match (nibble(bytes[index + 1]), nibble(bytes[index + 2])) {
                    (Some(high), Some(low)) => {
                        out.push(high << 4 | low);
                        index += 3;
                    }
                    _ => {
                        out.push(bytes[index]);
                        index += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn reply() -> Vec<u8> {
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n{CLOSED_PAGE}",
        CLOSED_PAGE.len()
    )
    .into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn raw_get(port: u16, target: &str) -> String {
        let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).await.expect("connect");
        let request = format!("GET {target} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n");
        stream.write_all(request.as_bytes()).await.expect("write");
        let mut response = String::new();
        stream.read_to_string(&mut response).await.expect("read");
        response
    }

    /// Nothing is listening on `port` any more, so the OS hands it straight back.
    /// (A connect() probe is unreliable here: Windows loopback drops the SYN instead
    /// of refusing it.)
    async fn assert_port_is_free(port: u16) {
        let rebind = tokio::time::timeout(
            Duration::from_secs(2),
            TcpListener::bind((Ipv4Addr::LOCALHOST, port)),
        )
        .await;
        assert!(matches!(rebind, Ok(Ok(_))), "the port must be free once the flow ends");
    }

    #[tokio::test]
    async fn binds_an_ephemeral_loopback_port() {
        let loopback = Loopback::bind().await.expect("bind");
        assert_ne!(loopback.port(), 0);
        assert_eq!(loopback.redirect_uri(), format!("http://127.0.0.1:{}", loopback.port()));
    }

    #[tokio::test]
    async fn serves_one_code_and_then_closes_the_port() {
        let loopback = Loopback::bind().await.expect("bind");
        let port = loopback.port();
        let state = State::generate().expect("state");
        let server = tokio::spawn({
            let state = state.clone();
            async move { loopback.wait_for_code(&state).await }
        });

        let response =
            raw_get(port, &format!("/?code=4%2F0abc&scope=drive&state={}", state.state())).await;
        assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
        assert!(response.contains("close this tab"), "{response}");

        assert_eq!(server.await.expect("join").expect("code"), "4/0abc");

        assert_port_is_free(port).await;
    }

    /// The give-up path (issue #22): a browser that never comes back must end the flow
    /// and release the port, or the app hangs forever with the socket held.
    #[tokio::test]
    async fn a_browser_that_never_returns_abandons_the_flow_and_frees_the_port() {
        let loopback = Loopback::bind_with_timeout(Duration::from_millis(100)).await.expect("bind");
        let port = loopback.port();
        let state = State::generate().expect("state");

        let err = loopback.wait_for_code(&state).await.expect_err("must give up");
        assert_eq!(err.code(), "cancelled");
        assert!(err.to_string().contains("never returned"), "{err}");

        assert_port_is_free(port).await;
    }

    #[tokio::test]
    async fn rejects_a_mismatched_state() {
        let loopback = Loopback::bind().await.expect("bind");
        let port = loopback.port();
        let state = State::generate().expect("state");
        let server = tokio::spawn({
            let state = state.clone();
            async move { loopback.wait_for_code(&state).await }
        });

        let _ = raw_get(port, "/?code=stolen&state=not-the-state").await;

        let err = server.await.expect("join").expect_err("must reject");
        assert_eq!(err.code(), "protocol");

        assert_port_is_free(port).await;
    }

    #[tokio::test]
    async fn a_denied_consent_screen_is_cancelled_not_a_protocol_error() {
        let loopback = Loopback::bind().await.expect("bind");
        let port = loopback.port();
        let state = State::generate().expect("state");
        let server = tokio::spawn({
            let state = state.clone();
            async move { loopback.wait_for_code(&state).await }
        });

        // The sentinel is planted where a server response can put a token; only
        // `error=` may reach the message (issue #25).
        let _ =
            raw_get(port, "/?error=access_denied&error_description=User+denied+ya29.PLANTED").await;

        let err = server.await.expect("join").expect_err("must stop");
        assert_eq!(err.code(), "cancelled");
        assert!(err.to_string().contains("access_denied"));
        for rendered in [err.to_string(), format!("{err:?}")] {
            assert!(!rendered.contains("PLANTED"), "the callback must not be echoed: {rendered}");
        }
    }

    #[test]
    fn a_callback_without_a_code_is_a_protocol_error() {
        let state = State::from_raw("abc");
        let err = parse_callback("GET /?state=abc HTTP/1.1\r\n\r\n", &state).expect_err("no code");
        assert_eq!(err.code(), "protocol");

        let err = parse_callback("GET /?code=abc HTTP/1.1\r\n\r\n", &state).expect_err("no state");
        assert_eq!(err.code(), "protocol");

        let err = parse_callback("nonsense\r\n\r\n", &state).expect_err("no target");
        assert_eq!(err.code(), "protocol");
    }

    #[test]
    fn decodes_percent_and_plus_escapes() {
        assert_eq!(decode_component("4%2F0abc"), "4/0abc");
        assert_eq!(decode_component("a+b"), "a b");
        assert_eq!(decode_component("%zz"), "%zz");
        assert_eq!(decode_component("plain"), "plain");
    }
}
