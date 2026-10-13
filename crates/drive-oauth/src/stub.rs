//! One-shot HTTP/1.1 stub: accept one connection, record the request, reply canned.
//! Test-only, so the flow can be exercised without touching Google.

use std::net::Ipv4Addr;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;

pub struct Stub {
    pub url: String,
    arrived: Option<oneshot::Receiver<Request>>,
}

pub struct Request {
    pub method: String,
    pub target: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Request {
    pub fn field(&self, name: &str) -> Option<String> {
        self.body
            .split('&')
            .filter_map(|pair| pair.split_once('='))
            .find(|(key, _)| decode(key) == name)
            .map(|(_, value)| decode(value))
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        let name = name.to_ascii_lowercase();
        self.headers.iter().find(|(key, _)| *key == name).map(|(_, value)| value.as_str())
    }
}

impl Stub {
    pub async fn start(status: u16, body: &str) -> Self {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.expect("stub bind");
        let port = listener.local_addr().expect("stub addr").port();
        let (sender, arrived) = oneshot::channel();
        let reply = format!(
            "HTTP/1.1 {status} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
             Connection: close\r\n\r\n{body}",
            reason(status),
            body.len()
        );

        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("stub accept");
            let request = read_request(&mut stream).await;
            let _ = stream.write_all(reply.as_bytes()).await;
            let _ = sender.send(request);
        });

        Self { url: format!("http://127.0.0.1:{port}"), arrived: Some(arrived) }
    }

    /// Waits for the recorded request. Panics if it was already taken.
    pub async fn request(mut self) -> Request {
        self.arrived.take().expect("stub request already taken").await.expect("stub task died")
    }
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        403 => "Forbidden",
        500 => "Internal Server Error",
        _ => "Status",
    }
}

async fn read_request(stream: &mut TcpStream) -> Request {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        let read = stream.read(&mut chunk).await.expect("stub read");
        if read == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..read]);
        let Some(head_end) = buf.windows(4).position(|window| window == b"\r\n\r\n") else {
            continue;
        };
        let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
        let length = content_length(&head);
        if buf.len() >= head_end + 4 + length {
            break;
        }
    }

    let head_end = buf.windows(4).position(|window| window == b"\r\n\r\n").unwrap_or(buf.len());
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let body = String::from_utf8_lossy(&buf[head_end + 4..]).into_owned();

    let mut lines = head.split("\r\n");
    let mut request_line = lines.next().unwrap_or_default().split_whitespace();
    let method = request_line.next().unwrap_or_default().to_string();
    let target = request_line.next().unwrap_or_default().to_string();
    let headers = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(key, value)| (key.trim().to_ascii_lowercase(), value.trim().to_string()))
        .collect();

    Request { method, target, headers, body }
}

fn content_length(head: &str) -> usize {
    head.split("\r\n")
        .filter_map(|line| line.split_once(':'))
        .find(|(key, _)| key.trim().eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.trim().parse().ok())
        .unwrap_or(0)
}

fn decode(raw: &str) -> String {
    percent_encoding_decode(raw)
}

fn percent_encoding_decode(raw: &str) -> String {
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
