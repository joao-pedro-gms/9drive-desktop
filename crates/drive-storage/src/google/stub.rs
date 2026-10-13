//! A local HTTP/1.1 stub: answers each request from a queue of canned replies and records
//! what it received. Test-only, so the provider can be exercised without touching Google.

use std::net::Ipv4Addr;
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

pub struct Stub {
    pub base: String,
    received: Arc<Mutex<Vec<Request>>>,
}

#[derive(Clone)]
pub struct Request {
    pub method: String,
    pub target: String,
    pub path: String,
    pub query: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        let name = name.to_ascii_lowercase();
        self.headers.iter().find(|(key, _)| *key == name).map(|(_, value)| value.as_str())
    }

    /// Percent-decoded query parameter, so a test can assert on the value Drive would see.
    pub fn query(&self, name: &str) -> Option<String> {
        self.query
            .split('&')
            .filter_map(|pair| pair.split_once('='))
            .find(|(key, _)| decode(key) == name)
            .map(|(_, value)| decode(value))
    }
}

#[derive(Clone)]
pub struct Reply {
    status: u16,
    body: Vec<u8>,
    headers: Vec<(String, String)>,
}

impl Reply {
    pub fn new(status: u16, body: &str) -> Self {
        Self::bytes(status, body.as_bytes().to_vec())
    }

    /// Byte-exact body for binary replies. [`Reply::new`] takes a `&str`, so it cannot carry
    /// a payload that is not valid UTF-8.
    pub fn bytes(status: u16, body: Vec<u8>) -> Self {
        Self { status, body, headers: Vec::new() }
    }

    pub fn json(status: u16, body: &str) -> Self {
        Self::new(status, body).header("Content-Type", "application/json; charset=UTF-8")
    }

    pub fn empty(status: u16) -> Self {
        Self::new(status, "")
    }

    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }
}

impl Stub {
    /// `replies` are served in order; the last one repeats once the queue runs dry, so a
    /// test can describe a retry sequence without counting its attempts.
    pub async fn start(replies: Vec<Reply>) -> Self {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.expect("stub bind");
        let port = listener.local_addr().expect("stub addr").port();
        let received = Arc::new(Mutex::new(Vec::new()));

        let recorder = Arc::clone(&received);
        tokio::spawn(async move {
            let mut index = 0usize;
            loop {
                let Ok((stream, _)) = listener.accept().await else { break };
                // An exhausted queue repeats the last reply rather than closing, so a
                // retry loop ends on a real response instead of a transport error.
                let reply = replies.get(index.min(replies.len().saturating_sub(1))).cloned();
                index += 1;
                let recorder = Arc::clone(&recorder);
                tokio::spawn(async move {
                    let Some(reply) = reply else { return };
                    let mut stream = stream;
                    if let Some(request) = read_request(&mut stream).await {
                        recorder.lock().expect("stub recorder").push(request);
                    }
                    // A client that stops reading mid-body (the 64 KiB error cap) resets the
                    // connection; that is the expected outcome here, not a failure.
                    let _ = stream.write_all(&render(&reply)).await;
                    let _ = stream.shutdown().await;
                });
            }
        });

        Self { base: format!("http://127.0.0.1:{port}"), received }
    }

    pub fn requests(&self) -> Vec<Request> {
        self.received.lock().expect("stub recorder").clone()
    }

    pub fn request_count(&self) -> usize {
        self.received.lock().expect("stub recorder").len()
    }
}

fn render(reply: &Reply) -> Vec<u8> {
    let mut out = format!(
        "HTTP/1.1 {} {}\r\nContent-Length: {}\r\nConnection: close\r\n",
        reply.status,
        reason(reply.status),
        reply.body.len()
    );
    for (name, value) in &reply.headers {
        out.push_str(&format!("{name}: {value}\r\n"));
    }
    out.push_str("\r\n");
    let mut bytes = out.into_bytes();
    bytes.extend_from_slice(&reply.body);
    bytes
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        204 => "No Content",
        308 => "Permanent Redirect",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        409 => "Conflict",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "Status",
    }
}

async fn read_request(stream: &mut TcpStream) -> Option<Request> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        let read = stream.read(&mut chunk).await.ok()?;
        if read == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..read]);
        if let Some(end) = buf.windows(4).position(|window| window == b"\r\n\r\n") {
            break end;
        }
    };

    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let length = content_length(&head);
    while buf.len() < head_end + 4 + length {
        match stream.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(read) => buf.extend_from_slice(&chunk[..read]),
        }
    }

    let body_end = (head_end + 4 + length).min(buf.len());
    let body = String::from_utf8_lossy(&buf[head_end + 4..body_end]).into_owned();

    let mut lines = head.split("\r\n");
    let mut request_line = lines.next().unwrap_or_default().split_whitespace();
    let method = request_line.next().unwrap_or_default().to_string();
    let target = request_line.next().unwrap_or_default().to_string();
    let (path, query) =
        target.split_once('?').map_or((target.as_str(), ""), |(path, query)| (path, query));
    let headers = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(key, value)| (key.trim().to_ascii_lowercase(), value.trim().to_string()))
        .collect();

    Some(Request {
        method,
        path: path.to_string(),
        query: query.to_string(),
        target,
        headers,
        body,
    })
}

fn content_length(head: &str) -> usize {
    head.split("\r\n")
        .filter_map(|line| line.split_once(':'))
        .find(|(key, _)| key.trim().eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.trim().parse().ok())
        .unwrap_or(0)
}

fn decode(raw: &str) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn records_the_method_path_query_headers_and_body() {
        let stub = Stub::start(vec![Reply::empty(204)]).await;
        let response = reqwest::Client::new()
            .post(format!("{}/files?q=a%20b&pageSize=10", stub.base))
            .header("X-Test", "yes")
            .body("{\"name\":\"9drive\"}")
            .send()
            .await
            .expect("post");

        assert_eq!(response.status(), 204);
        let request = &stub.requests()[0];
        assert_eq!(request.method, "POST");
        assert_eq!(request.path, "/files");
        assert_eq!(request.query("q").as_deref(), Some("a b"));
        assert_eq!(request.query("pageSize").as_deref(), Some("10"));
        assert_eq!(request.header("x-test"), Some("yes"));
        assert_eq!(request.body, "{\"name\":\"9drive\"}");
        assert_eq!(stub.request_count(), 1);
    }

    #[tokio::test]
    async fn the_last_reply_repeats_once_the_queue_runs_dry() {
        let stub = Stub::start(vec![Reply::new(500, "boom")]).await;
        for _ in 0..3 {
            assert_eq!(reqwest::get(&stub.base).await.expect("get").status(), 500);
        }
        assert_eq!(stub.request_count(), 3);
    }
}
