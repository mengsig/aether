#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! A loopback HTTP server that replays one canned response, and records what
//! the adapter sent. Enough HTTP/1.1 for the adapters under test, no more.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

/// What the fixture server answers with.
#[derive(Debug, Clone)]
pub enum Canned {
    /// A 200 with an event stream body.
    Sse(String),
    Error {
        status: u16,
        body: String,
        retry_after_secs: Option<u64>,
    },
}

impl Canned {
    pub fn sse(body: impl Into<String>) -> Self {
        Self::Sse(body.into())
    }

    pub fn error(status: u16, body: impl Into<String>, retry_after_secs: Option<u64>) -> Self {
        Self::Error {
            status,
            body: body.into(),
            retry_after_secs,
        }
    }

    fn render(&self) -> Vec<u8> {
        match self {
            Self::Sse(body) => {
                let head = "HTTP/1.1 200 OK\r\n\
                     content-type: text/event-stream\r\n\
                     cache-control: no-cache\r\n\
                     request-id: req_conformance\r\n\
                     connection: close\r\n\r\n";
                [head.as_bytes(), body.as_bytes()].concat()
            }
            Self::Error {
                status,
                body,
                retry_after_secs,
            } => {
                let mut head = format!(
                    "HTTP/1.1 {status} Error\r\n\
                     content-type: application/json\r\n\
                     request-id: req_conformance\r\n\
                     content-length: {}\r\n",
                    body.len()
                );
                if let Some(secs) = retry_after_secs {
                    head.push_str(&format!("retry-after: {secs}\r\n"));
                }
                head.push_str("connection: close\r\n\r\n");
                [head.as_bytes(), body.as_bytes()].concat()
            }
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct RecordedRequest {
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl RecordedRequest {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

#[derive(Debug)]
pub struct FixtureServer {
    addr: SocketAddr,
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
    accept: JoinHandle<()>,
}

impl FixtureServer {
    /// Bind an ephemeral loopback port and answer every request with `canned`.
    pub async fn start(canned: Canned) -> Self {
        let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
            .await
            .expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let sink = requests.clone();
        let accept = tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                let canned = canned.clone();
                let sink = sink.clone();
                tokio::spawn(async move { serve(socket, &canned, &sink).await });
            }
        });
        Self {
            addr,
            requests,
            accept,
        }
    }

    pub fn base_url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// The request the adapter sent. Panics if it sent none.
    pub fn take_request(&self) -> RecordedRequest {
        self.requests
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .first()
            .cloned()
            .expect("the adapter sent no request")
    }

    pub fn requests(&self) -> Vec<RecordedRequest> {
        self.requests
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }
}

impl Drop for FixtureServer {
    fn drop(&mut self) {
        self.accept.abort();
    }
}

/// Records what the client sent as soon as it is parsed, *before* answering:
/// after the response the client can already have seen EOF and returned, so a
/// caller reading `take_request` would race the recording.
async fn serve(
    mut socket: TcpStream,
    canned: &Canned,
    sink: &Mutex<Vec<RecordedRequest>>,
) -> Option<()> {
    let mut buf = Vec::new();
    let head_end = loop {
        if let Some(at) = find(&buf, b"\r\n\r\n") {
            break at + 4;
        }
        let mut chunk = [0u8; 4096];
        let n = socket.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let mut lines = head.lines();
    let path = lines
        .next()?
        .split_whitespace()
        .nth(1)
        .unwrap_or("")
        .to_owned();
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_lowercase(), v.trim().to_owned()))
        .collect();
    let length: usize = headers
        .iter()
        .find(|(k, _)| k == "content-length")
        .and_then(|(_, v)| v.parse().ok())
        .unwrap_or(0);
    let mut body = buf.split_off(head_end);
    while body.len() < length {
        let mut chunk = [0u8; 4096];
        let n = socket.read(&mut chunk).await.ok()?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    sink.lock()
        .unwrap_or_else(|p| p.into_inner())
        .push(RecordedRequest {
            path,
            headers,
            body: String::from_utf8_lossy(&body).into_owned(),
        });
    socket.write_all(&canned.render()).await.ok()?;
    socket.flush().await.ok()?;
    // The SSE body ends at EOF; the adapter must see the close.
    socket.shutdown().await.ok();
    Some(())
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}
