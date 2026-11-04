//! The mount over real loopback sockets: a fake archond behind it, and a publication status
//! the test turns from private to Funnel, failing or hanging.

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use aether_protocol::remote::{DeviceRole, MountKind, RemoteMount, RemoteSettings};
use bytes::Bytes;
use futures::{SinkExt as _, StreamExt as _};
use http_body_util::{BodyExt as _, Full};
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;
use tokio_tungstenite::tungstenite::protocol::Role;
use tokio_util::sync::CancellationToken;

use super::{Publication, end_to_end, find, target_address};
use crate::testing::{self, EchoHost, MemoryDevices};
use crate::{DeviceGate, Ended, RemoteHost, RemoteServer, Tuning};

const PUBLIC: &str = "https://host.tailnet.example";
const PUBLIC_HOST: &str = "host.tailnet.example";
/// "Within 5 s", with room for the close to cross loopback on a busy machine.
/// What a tunnel closed for its lease ends with.
const POLICY: u16 = 1008;
const WITHIN_THE_LEASE: Duration = Duration::from_millis(5_300);

/// What the fake status read answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Status {
    Private,
    Funnel,
    Failing,
    Hanging,
}

/// A daemon whose publication status the test sets; it serves devices as [`EchoHost`] does.
#[derive(Debug)]
pub(crate) struct StatusHost {
    status: parking_lot::Mutex<Status>,
    pub reads: AtomicUsize,
}

impl StatusHost {
    pub(crate) fn new(status: Status) -> Arc<Self> {
        Arc::new(Self {
            status: parking_lot::Mutex::new(status),
            reads: AtomicUsize::new(0),
        })
    }

    pub(crate) fn set(&self, status: Status) {
        *self.status.lock() = status;
    }
}

#[async_trait::async_trait]
impl RemoteHost for StatusHost {
    fn instance_id(&self) -> &str {
        testing::INSTANCE
    }

    async fn serve(
        &self,
        stream: tokio::io::DuplexStream,
        gate: DeviceGate,
        shutdown: CancellationToken,
    ) -> anyhow::Result<Ended> {
        EchoHost.serve(stream, gate, shutdown).await
    }

    async fn publication(&self) -> Result<Publication, String> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        let status = *self.status.lock();
        match status {
            Status::Private => Ok(Publication::Private),
            Status::Funnel => Ok(Publication::Funnel),
            Status::Failing => Err("tailscaled is not answering".into()),
            Status::Hanging => std::future::pending().await,
        }
    }
}

/// What the fake archond was sent.
#[derive(Debug, Default)]
struct Seen {
    requests: parking_lot::Mutex<Vec<SeenRequest>>,
    frames: parking_lot::Mutex<Vec<String>>,
}

#[derive(Debug, Clone)]
struct SeenRequest {
    method: String,
    uri: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

/// A loopback archond: answers requests with what it was asked, and echoes WebSocket text.
async fn archond() -> (SocketAddr, Arc<Seen>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("address");
    let seen = Arc::new(Seen::default());
    let serving = seen.clone();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let seen = serving.clone();
            tokio::spawn(async move {
                let service = service_fn(move |request| answer(request, seen.clone()));
                let connection = hyper::server::conn::http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), service)
                    .with_upgrades();
                if let Err(error) = connection.await {
                    tracing::debug!(%error, "the fake archond's connection failed");
                }
            });
        }
    });
    (address, seen)
}

async fn answer(
    mut request: Request<Incoming>,
    seen: Arc<Seen>,
) -> Result<Response<Full<Bytes>>, Infallible> {
    let headers = request
        .headers()
        .iter()
        .map(|(name, value)| (name.to_string(), value.to_str().unwrap_or("?").to_owned()))
        .collect();
    let upgrade = request.headers().contains_key("upgrade");
    let key = request.headers().get("sec-websocket-key").cloned();
    let on_upgrade = upgrade.then(|| hyper::upgrade::on(&mut request));
    let (parts, body) = request.into_parts();
    let body = body
        .collect()
        .await
        .map(|b| b.to_bytes().to_vec())
        .unwrap_or_default();
    seen.requests.lock().push(SeenRequest {
        method: parts.method.to_string(),
        uri: parts.uri.to_string(),
        headers,
        body,
    });
    let mut response = Response::new(Full::new(Bytes::from(format!("archond {}", parts.uri))));
    let (Some(on_upgrade), Some(key)) = (on_upgrade, key) else {
        let headers = response.headers_mut();
        let cookie = "archon_device=1; Path=/archon; HttpOnly";
        headers.insert("set-cookie", cookie.parse().expect("cookie"));
        headers.insert(
            "content-security-policy",
            "default-src 'none'".parse().expect("csp"),
        );
        return Ok(response);
    };
    tokio::spawn(echo(on_upgrade, seen));
    let accept = crate::ws::accept_key(key.as_bytes());
    *response.status_mut() = StatusCode::SWITCHING_PROTOCOLS;
    let headers = response.headers_mut();
    headers.insert("connection", "upgrade".parse().expect("token"));
    headers.insert("upgrade", "websocket".parse().expect("token"));
    headers.insert("sec-websocket-accept", accept.parse().expect("accept"));
    Ok(response)
}

async fn echo(on_upgrade: hyper::upgrade::OnUpgrade, seen: Arc<Seen>) {
    let Ok(upgraded) = on_upgrade.await else {
        return;
    };
    let mut socket =
        WebSocketStream::from_raw_socket(TokioIo::new(upgraded), Role::Server, None).await;
    while let Some(Ok(message)) = socket.next().await {
        match message {
            Message::Text(text) => {
                seen.frames.lock().push(text.to_string());
                if socket
                    .send(Message::text(format!("echo:{text}")))
                    .await
                    .is_err()
                {
                    return;
                }
            }
            Message::Close(frame) => {
                let code = frame.map_or(0, |frame| u16::from(frame.code));
                seen.frames.lock().push(format!("close {code}"));
                return;
            }
            _ => {}
        }
    }
}

fn archon_mount(target: SocketAddr) -> RemoteMount {
    RemoteMount {
        prefix: "/archon".into(),
        kind: MountKind::Archon,
        target: format!("http://{target}"),
    }
}

fn settings(mounts: Option<Vec<RemoteMount>>) -> RemoteSettings {
    RemoteSettings {
        enabled: true,
        pairing: true,
        public_origins: vec![PUBLIC.into()],
        push: false,
        mounts,
    }
}

struct Mounted {
    server: Arc<RemoteServer>,
    bound: SocketAddr,
    host: Arc<StatusHost>,
    devices: Arc<MemoryDevices>,
    _shutdown: tokio_util::sync::DropGuard,
}

/// A listener mounting `/archon` on `target`, its status private to start with.
async fn mounted(target: SocketAddr) -> Mounted {
    let host = StatusHost::new(Status::Private);
    let devices = Arc::new(MemoryDevices::default());
    let shutdown = CancellationToken::new();
    let server = RemoteServer::with_tuning(
        testing::config(),
        Tuning::default(),
        host.clone(),
        devices.clone(),
        shutdown.clone(),
    )
    .expect("a loopback listener");
    let server = Arc::new(server);
    server
        .apply(settings(Some(vec![archon_mount(target)])))
        .await
        .expect("apply");
    let bound = server.listening().expect("open");
    Mounted {
        server,
        bound,
        host,
        devices,
        _shutdown: shutdown.drop_guard(),
    }
}

/// Send raw bytes and read the whole answer, which the listener ends by closing.
async fn raw(bound: SocketAddr, request: &str) -> (u16, Vec<(String, String)>, String) {
    let mut stream = TcpStream::connect(bound).await.expect("connect");
    stream.write_all(request.as_bytes()).await.expect("write");
    let mut answer = String::new();
    let reading = tokio::time::timeout(Duration::from_secs(10), stream.read_to_string(&mut answer));
    reading.await.expect("an answer in time").expect("read");
    let (head, body) = answer.split_once("\r\n\r\n").expect("a head");
    let mut lines = head.lines();
    let status = lines
        .next()
        .expect("a status line")
        .split(' ')
        .nth(1)
        .expect("a code");
    let headers = lines
        .filter_map(|line| line.split_once(": "))
        .map(|(name, value)| (name.to_ascii_lowercase(), value.to_owned()))
        .collect();
    (status.parse().expect("a number"), headers, body.to_owned())
}

fn get(path: &str) -> String {
    format!("GET {path} HTTP/1.1\r\nHost: {PUBLIC_HOST}\r\nConnection: close\r\n\r\n")
}

fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, v)| v.as_str())
}

/// A WebSocket through the mount, as a page on the public origin opens one.
async fn tunnel(bound: SocketAddr) -> WebSocketStream<TcpStream> {
    let mut request = format!("ws://{bound}/archon/ws")
        .into_client_request()
        .expect("request");
    let headers = request.headers_mut();
    headers.insert("host", PUBLIC_HOST.parse().expect("host"));
    headers.insert("origin", PUBLIC.parse().expect("origin"));
    headers.insert(
        "sec-websocket-extensions",
        "permessage-deflate".parse().expect("ext"),
    );
    let stream = TcpStream::connect(bound).await.expect("connect");
    let (mut socket, response) = tokio_tungstenite::client_async(request, stream)
        .await
        .expect("the target's upgrade is passed on");
    assert_eq!(response.status(), StatusCode::SWITCHING_PROTOCOLS);
    socket.send(Message::text("hi")).await.expect("send");
    assert_eq!(next_text(&mut socket).await, "echo:hi");
    socket
}

async fn next_text(socket: &mut WebSocketStream<TcpStream>) -> String {
    loop {
        let next = tokio::time::timeout(Duration::from_secs(10), socket.next()).await;
        match next
            .expect("a message in time")
            .expect("open")
            .expect("a frame")
        {
            Message::Text(text) => return text.to_string(),
            Message::Ping(_) | Message::Pong(_) => {}
            other => panic!("expected text, got {other:?}"),
        }
    }
}

/// The close code the tunnel ends with, and how long it took.
async fn closed_with(socket: &mut WebSocketStream<TcpStream>) -> (u16, Duration) {
    let started = Instant::now();
    loop {
        let next = tokio::time::timeout(Duration::from_secs(10), socket.next()).await;
        match next
            .expect("a close in time")
            .expect("a close before the end")
        {
            Ok(Message::Close(frame)) => {
                return (u16::from(frame.expect("a code").code), started.elapsed());
            }
            Ok(_) => {}
            Err(error) => panic!("the tunnel failed before closing: {error}"),
        }
    }
}

#[test]
fn only_the_prefix_and_what_is_below_it_are_mounted() {
    let mounts = [archon_mount("127.0.0.1:8378".parse().expect("addr"))];
    let at =
        |path: &str, query| find(&mounts, path, query).map(|found| found.map(|m| m.path_and_query));
    assert_eq!(at("/archon", None), Some(Ok("/".into())));
    assert_eq!(at("/archon/", None), Some(Ok("/".into())));
    assert_eq!(
        at("/archon/api/x", Some("a=%2F")),
        Some(Ok("/api/x?a=%2F".into()))
    );
    assert_eq!(at("/archonx", None), None);
    assert_eq!(at("/arch", None), None);
    for bad in [
        "/archon/../x",
        "/archon/./x",
        "/archon//x",
        "/archon/a%2e",
        "/archon/a\\b",
    ] {
        assert_eq!(at(bad, None), Some(Err(super::BadPath)), "{bad}");
    }
}

#[test]
fn a_target_is_used_only_when_it_is_exactly_loopback_http() {
    assert_eq!(
        target_address("http://127.0.0.1:8378"),
        Some("127.0.0.1:8378".parse().expect("addr"))
    );
    assert_eq!(
        target_address("http://[::1]:8378"),
        Some("[::1]:8378".parse().expect("addr"))
    );
    for refused in [
        "http://10.0.0.1:80",
        "http://127.0.0.1:0",
        "http://127.0.0.1:08378",
        "https://127.0.0.1:1",
        "http://localhost:1",
    ] {
        assert_eq!(target_address(refused), None, "{refused}");
    }
}

#[test]
fn hop_by_hop_headers_and_those_connection_names_stay_behind() {
    let mut headers = hyper::HeaderMap::new();
    for (name, value) in [
        ("connection", "keep-alive, x-hop"),
        ("keep-alive", "timeout=5"),
        ("te", "trailers"),
        ("transfer-encoding", "chunked"),
        ("upgrade", "h2c"),
        ("proxy-authorization", "Basic x"),
        ("x-hop", "secret"),
        ("origin", PUBLIC),
        ("cookie", "a=1"),
        ("cookie", "b=2"),
    ] {
        headers.append(name, value.parse().expect("value"));
    }
    let kept = end_to_end(&headers);
    let names: Vec<&str> = kept.keys().map(|name| name.as_str()).collect();
    assert_eq!(names, ["origin", "cookie"]);
    assert_eq!(
        kept.get_all("cookie").iter().count(),
        2,
        "repeated headers stay whole"
    );
}

#[tokio::test]
async fn more_than_four_mounts_or_one_off_loopback_is_refused_and_changes_nothing() {
    let (server, _devices, _shutdown) = testing::server(Tuning::default());
    let five = ["/a", "/b", "/c", "/d", "/e"].map(|prefix| RemoteMount {
        prefix: prefix.into(),
        ..archon_mount("127.0.0.1:1".parse().expect("addr"))
    });
    assert!(server.apply(settings(Some(five.to_vec()))).await.is_err());
    let outside = RemoteMount {
        target: "http://192.0.2.1:8378".into(),
        ..five[0].clone()
    };
    assert!(server.apply(settings(Some(vec![outside]))).await.is_err());
    assert_eq!(
        server.settings(),
        RemoteSettings::default(),
        "nothing was applied"
    );
    assert!(server.listening().is_none());
}

#[tokio::test]
async fn a_mounted_request_reaches_the_target_with_only_host_changed_and_an_observer_adds_nothing()
{
    let (target, seen) = archond().await;
    let mounted = mounted(target).await;
    // An observer device holds a session on Aether, as a paired browser would.
    let key = testing::key_pair();
    let mut observer = testing::record(&key);
    observer.role = DeviceRole::Observer;
    mounted.devices.put(&observer);
    let _session = observer_session(mounted.bound, &key, &observer).await;

    let request = format!(
        "POST /archon/api/pair/claim?x=1 HTTP/1.1\r\nHost: {PUBLIC_HOST}\r\nOrigin: {PUBLIC}\r\n\
         Sec-Fetch-Site: same-origin\r\nCookie: archon_device=abc\r\nContent-Type: application/json\r\n\
         Content-Length: 2\r\nConnection: keep-alive, x-hop\r\nX-Hop: secret\r\nKeep-Alive: timeout=5\r\n\
         TE: trailers\r\nX-Custom: 1\r\n\r\n{{}}"
    );
    let (status, headers, body) = raw(mounted.bound, &request).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body, "archond /api/pair/claim?x=1");
    let sent = seen.requests.lock().clone();
    let [only] = sent.as_slice() else {
        panic!("one request reached archond, not {sent:?}");
    };
    assert_eq!(
        (only.method.as_str(), only.uri.as_str()),
        ("POST", "/api/pair/claim?x=1")
    );
    let expected = [
        ("host", target.to_string().as_str()),
        ("origin", PUBLIC),
        ("sec-fetch-site", "same-origin"),
        ("cookie", "archon_device=abc"),
        ("content-type", "application/json"),
        ("content-length", "2"),
        ("x-custom", "1"),
    ]
    .map(|(name, value)| (name.to_owned(), value.to_owned()));
    let mut got = only.headers.clone();
    got.sort();
    let mut want = expected.to_vec();
    want.sort();
    assert_eq!(
        got, want,
        "exactly the client's end-to-end headers, Host aside"
    );
    assert_eq!(only.body, b"{}");
    assert_passed_back(&headers);
}

/// The target's own headers come back as they are; the listener's only fill gaps.
fn assert_passed_back(headers: &[(String, String)]) {
    let cookie = "archon_device=1; Path=/archon; HttpOnly";
    assert_eq!(header(headers, "set-cookie"), Some(cookie));
    let csp = header(headers, "content-security-policy");
    assert_eq!(csp, Some("default-src 'none'"), "the target's own policy");
    let nosniff = header(headers, "x-content-type-options");
    assert_eq!(nosniff, Some("nosniff"), "filled in");
    let connection = header(headers, "connection");
    assert_eq!(
        connection,
        Some("close"),
        "no kept-alive mounted connection"
    );
}

/// An authenticated device session on Aether's own socket.
async fn observer_session(
    bound: SocketAddr,
    key: &aws_lc_rs::signature::EcdsaKeyPair,
    record: &crate::auth::DeviceRecord,
) -> WebSocketStream<TcpStream> {
    let mut request = format!("ws://{bound}/ws")
        .into_client_request()
        .expect("request");
    let headers = request.headers_mut();
    headers.insert("host", PUBLIC_HOST.parse().expect("host"));
    headers.insert("origin", PUBLIC.parse().expect("origin"));
    headers.insert("sec-websocket-protocol", "aether".parse().expect("token"));
    let stream = TcpStream::connect(bound).await.expect("connect");
    let (mut socket, _) = tokio_tungstenite::client_async(request, stream)
        .await
        .expect("upgrade");
    let challenge = next_text(&mut socket).await;
    let nonce = challenge.strip_prefix("challenge ").expect("a challenge");
    let hello = testing::hello(key, record, PUBLIC, nonce);
    socket.send(Message::text(hello)).await.expect("hello");
    assert_eq!(next_text(&mut socket).await, "welcome phone");
    socket
}

#[tokio::test]
async fn paths_under_a_mount_that_are_not_clean_are_refused_and_never_forwarded() {
    let (target, seen) = archond().await;
    let mounted = mounted(target).await;
    for bad in [
        "/archon/../x",
        "/archon/./x",
        "/archon//x",
        "/archon/a%2e%2e",
        "/archon/a\\b",
    ] {
        let (status, _, body) = raw(mounted.bound, &get(bad)).await;
        assert_eq!(status, 400, "{bad}: {body}");
    }
    raw(mounted.bound, &get("/archonx")).await;
    assert!(seen.requests.lock().is_empty(), "nothing reached archond");
    let (status, _, body) = raw(mounted.bound, &get("/archon")).await;
    assert_eq!((status, body.as_str()), (200, "archond /"));
}

#[tokio::test]
async fn a_body_over_the_listener_limit_is_refused_and_never_forwarded() {
    let (target, seen) = archond().await;
    let mounted = mounted(target).await;
    let length = crate::http::MAX_BODY_BYTES + 1;
    let request = format!(
        "POST /archon/api/x HTTP/1.1\r\nHost: {PUBLIC_HOST}\r\nContent-Length: {length}\r\n\
         Connection: close\r\n\r\n{}",
        "x".repeat(length)
    );
    let (status, _, _) = raw(mounted.bound, &request).await;
    assert_eq!(status, 413);
    assert!(seen.requests.lock().is_empty());
}

#[tokio::test]
async fn a_target_that_is_down_answers_502_at_once() {
    let free = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let target = free.local_addr().expect("address");
    drop(free);
    let mounted = mounted(target).await;
    let started = Instant::now();
    let (status, _, body) = raw(mounted.bound, &get("/archon/api/health")).await;
    assert_eq!(status, 502, "{body}");
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn health_lists_the_mounts_by_prefix_and_kind_but_not_where_they_lead() {
    let (target, _seen) = archond().await;
    let mounted = mounted(target).await;
    let (status, _, body) = raw(mounted.bound, &get("/api/health")).await;
    assert_eq!(status, 200);
    let health: serde_json::Value = serde_json::from_str(&body).expect("json");
    assert_eq!(
        health["mounts"],
        serde_json::json!([{"prefix": "/archon", "kind": "archon"}])
    );
    assert!(!body.contains(&target.port().to_string()), "{body}");
    assert_eq!(health["status"], "ok");
}

#[tokio::test]
async fn a_tunnel_relays_frames_and_close_codes_and_keeps_origin() {
    let (target, seen) = archond().await;
    let mounted = mounted(target).await;
    let mut socket = tunnel(mounted.bound).await;
    socket.send(Message::text("again")).await.expect("send");
    assert_eq!(next_text(&mut socket).await, "echo:again");
    let leaving = tokio_tungstenite::tungstenite::protocol::CloseFrame {
        code: 4000.into(),
        reason: "bye".into(),
    };
    socket.close(Some(leaving)).await.expect("close");
    let upgrade = seen.requests.lock()[0].clone();
    assert_eq!(upgrade.uri, "/ws");
    let headers: std::collections::BTreeMap<_, _> = upgrade.headers.into_iter().collect();
    assert_eq!(headers.get("origin").map(String::as_str), Some(PUBLIC));
    assert!(
        !headers.contains_key("sec-websocket-extensions"),
        "frames are relayed as they are"
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while !seen.frames.lock().contains(&"close 4000".to_owned()) {
        assert!(
            Instant::now() < deadline,
            "the close code reached archond: {:?}",
            seen.frames.lock()
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(*seen.frames.lock(), ["hi", "again", "close 4000"]);
}

#[tokio::test]
async fn an_out_of_band_funnel_closes_tunnels_with_1008_and_refuses_requests_within_5s() {
    let (target, seen) = archond().await;
    let mounted = mounted(target).await;
    let mut socket = tunnel(mounted.bound).await;
    mounted.host.set(Status::Funnel);
    let (code, took) = closed_with(&mut socket).await;
    assert_eq!(code, POLICY);
    assert!(took < WITHIN_THE_LEASE, "{took:?}");
    let started = Instant::now();
    let (status, _, body) = raw(mounted.bound, &get("/archon/api/x")).await;
    assert_eq!(status, 503, "{body}");
    assert!(body.contains("Funnel"), "{body}");
    assert!(started.elapsed() < WITHIN_THE_LEASE);
    let reached = seen.frames.lock().clone();
    assert_eq!(
        reached,
        ["hi"],
        "nothing, not even a close, reached archond after the lease"
    );
    assert_eq!(
        seen.requests.lock().len(),
        1,
        "only the upgrade reached archond"
    );
}

#[tokio::test]
async fn a_failing_or_hanging_status_read_closes_the_tunnel_within_5s() {
    for broken in [Status::Failing, Status::Hanging] {
        let (target, seen) = archond().await;
        let mounted = mounted(target).await;
        let mut socket = tunnel(mounted.bound).await;
        mounted.host.set(broken);
        let (code, took) = closed_with(&mut socket).await;
        assert_eq!(code, POLICY, "{broken:?}");
        assert!(took < WITHIN_THE_LEASE, "{broken:?}: {took:?}");
        assert_eq!(*seen.frames.lock(), ["hi"], "{broken:?}");
    }
}

#[tokio::test]
async fn changing_the_mounts_closes_the_tunnels_opened_under_them() {
    let (target, seen) = archond().await;
    let mounted = mounted(target).await;
    let mut socket = tunnel(mounted.bound).await;
    mounted.server.apply(settings(None)).await.expect("unmount");
    let (code, _) = closed_with(&mut socket).await;
    assert_eq!(code, crate::ws::close::GOING_AWAY);
    raw(mounted.bound, &get("/archon/api/x")).await;
    assert_eq!(
        seen.requests.lock().len(),
        1,
        "no longer mounted: only the upgrade arrived"
    );
}

#[tokio::test]
async fn an_idle_listener_reads_the_status_only_when_traffic_asks() {
    let (target, _seen) = archond().await;
    let mounted = mounted(target).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        mounted.host.reads.load(Ordering::SeqCst),
        0,
        "no traffic, no reads"
    );
    let (status, _, _) = raw(mounted.bound, &get("/archon/x")).await;
    assert_eq!(status, 200);
    assert!(
        mounted.host.reads.load(Ordering::SeqCst) >= 1,
        "admission read the status"
    );
}
