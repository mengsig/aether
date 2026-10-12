//! The listener over real sockets: what a browser, or something that is
//! not one, sees on the wire.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use aether_protocol::remote::RemoteSettings;
use futures::{SinkExt as _, StreamExt as _};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;
use tokio_tungstenite::tungstenite::handshake::client::Request;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_util::sync::CancellationToken;

use crate::ws::close;
use crate::{RemoteServer, Tuning, testing};

const PUBLIC: &str = "https://host.tailnet.example";
const PUBLIC_HOST: &str = "host.tailnet.example";

fn settings(public_origins: &[&str]) -> RemoteSettings {
    RemoteSettings {
        enabled: true,
        pairing: true,
        public_origins: public_origins.iter().map(|o| (*o).to_owned()).collect(),
        push: false,
    }
}

async fn listening(tuning: Tuning) -> (Arc<RemoteServer>, SocketAddr, CancellationToken) {
    let (server, _devices, shutdown) = testing::server(tuning);
    server.apply(settings(&[PUBLIC])).await.expect("apply");
    let bound = server.listening().expect("the listener is open");
    (server, bound, shutdown)
}

/// Send raw bytes and read the whole answer; the request asks to close.
async fn raw(bound: SocketAddr, request: &str) -> String {
    let mut stream = TcpStream::connect(bound).await.expect("connect");
    stream.write_all(request.as_bytes()).await.expect("write");
    let mut answer = String::new();
    let reading = stream.read_to_string(&mut answer);
    let read = tokio::time::timeout(Duration::from_secs(10), reading).await;
    read.expect("an answer in time").expect("read");
    answer
}

/// An upgrade request as a page at `origin` reaching the listener by `host`.
fn upgrade(bound: SocketAddr, host: &str, origin: &str) -> Request {
    let mut request = format!("ws://{bound}/ws")
        .into_client_request()
        .expect("a request");
    let headers = request.headers_mut();
    headers.insert("host", host.parse().expect("host"));
    headers.insert("origin", origin.parse().expect("origin"));
    headers.insert("sec-websocket-protocol", "aether".parse().expect("token"));
    request
}

async fn connect(bound: SocketAddr, host: &str, origin: &str) -> WebSocketStream<TcpStream> {
    challenged(bound, host, origin).await.0
}

/// An open socket, and the nonce it was challenged with.
async fn challenged(
    bound: SocketAddr,
    host: &str,
    origin: &str,
) -> (WebSocketStream<TcpStream>, String) {
    let stream = TcpStream::connect(bound).await.expect("connect");
    let (mut socket, response) =
        tokio_tungstenite::client_async(upgrade(bound, host, origin), stream)
            .await
            .expect("the upgrade is accepted");
    assert_eq!(response.headers()["sec-websocket-protocol"], "aether");
    let challenge = text(&mut socket).await;
    let nonce = challenge.strip_prefix("challenge ").expect("a challenge");
    (socket, nonce.to_owned())
}

async fn text(socket: &mut WebSocketStream<TcpStream>) -> String {
    loop {
        let next = tokio::time::timeout(Duration::from_secs(10), socket.next()).await;
        match next
            .expect("a message in time")
            .expect("still open")
            .expect("a frame")
        {
            Message::Text(text) => return text.to_string(),
            Message::Ping(_) | Message::Pong(_) => {}
            other => panic!("expected text, got {other:?}"),
        }
    }
}

/// The code the connection is closed with.
async fn close_code(socket: &mut WebSocketStream<TcpStream>) -> u16 {
    loop {
        let next = tokio::time::timeout(Duration::from_secs(10), socket.next()).await;
        match next
            .expect("a close in time")
            .expect("a close before the end")
        {
            Ok(Message::Close(frame)) => return u16::from(frame.expect("a close code").code),
            Ok(_) => {}
            Err(error) => panic!("the connection failed before closing: {error}"),
        }
    }
}

/// Close from the client's side and see the close answered.
async fn leave(socket: &mut WebSocketStream<TcpStream>) {
    let leaving = CloseFrame {
        code: close::NORMAL.into(),
        reason: "".into(),
    };
    socket.close(Some(leaving)).await.expect("close");
    assert_eq!(close_code(socket).await, close::NORMAL);
}

#[tokio::test]
async fn http_is_served_on_the_bound_address_with_keep_alive() {
    let (_server, bound, _shutdown) = listening(Tuning::default()).await;
    let health = format!("GET /api/health HTTP/1.1\r\nHost: {bound}\r\n\r\n");
    let shell = format!("GET / HTTP/1.1\r\nHost: {bound}\r\nConnection: close\r\n\r\n");
    let answer = raw(bound, &format!("{health}{shell}")).await;
    assert!(answer.starts_with("HTTP/1.1 200 OK\r\n"), "{answer}");
    assert_eq!(
        answer.matches("HTTP/1.1 200 OK").count(),
        2,
        "both requests answered"
    );
    assert!(answer.contains("\"status\":\"ok\""), "{answer}");
    assert!(answer.contains("<!doctype html>"), "{answer}");
    let csp = "content-security-policy: default-src 'none'";
    assert_eq!(answer.to_lowercase().matches(csp).count(), 2, "{answer}");

    let rebound = raw(
        bound,
        "GET / HTTP/1.1\r\nHost: evil.example\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        rebound.starts_with("HTTP/1.1 403 Forbidden\r\n"),
        "{rebound}"
    );
    assert!(rebound.contains("forbidden_host"), "{rebound}");
    let garbage = raw(bound, "NOT HTTP AT ALL\r\n\r\n").await;
    assert!(garbage.starts_with("HTTP/1.1 400"), "{garbage}");
}

#[tokio::test]
async fn a_client_that_sends_no_request_is_hung_up_on() {
    let tuning = Tuning {
        header_timeout: Duration::from_millis(150),
        ..Tuning::default()
    };
    let (_server, bound, _shutdown) = listening(tuning).await;
    let started = tokio::time::Instant::now();
    let answer = raw(bound, "GET /api/health HTTP/1.1\r\nHost: ").await;
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "cut off at the header timeout"
    );
    assert!(!answer.contains("200 OK"), "{answer}");
}

#[tokio::test]
async fn a_websocket_carries_the_protocol_and_refuses_strangers() {
    let (_server, bound, _shutdown) = listening(Tuning::default()).await;
    let mut socket = connect(bound, &bound.to_string(), &format!("http://{bound}")).await;
    socket.send(Message::text("over tcp")).await.expect("send");
    assert_eq!(text(&mut socket).await, "echo over tcp");
    leave(&mut socket).await;

    for (host, origin, status) in [
        (bound.to_string(), "https://evil.example".to_owned(), 403),
        ("evil.example".to_owned(), format!("http://{bound}"), 403),
    ] {
        let stream = TcpStream::connect(bound).await.expect("connect");
        let refused = tokio_tungstenite::client_async(upgrade(bound, &host, &origin), stream).await;
        match refused.expect_err("a stranger is not upgraded") {
            tokio_tungstenite::tungstenite::Error::Http(response) => {
                assert_eq!(response.status().as_u16(), status, "{host} {origin}");
            }
            other => panic!("expected an HTTP refusal, got {other}"),
        }
    }
}

#[tokio::test]
async fn sockets_that_have_only_just_arrived_are_not_displaced() {
    let tuning = Tuning {
        lobby_patience: Duration::from_secs(60),
        ..Tuning::default()
    };
    let (server, bound, _shutdown) = listening(tuning).await;
    let (host, origin) = (bound.to_string(), format!("http://{bound}"));
    let mut open = Vec::new();
    for _ in 0..server.config().max_connections {
        open.push(connect(bound, &host, &origin).await);
    }
    let stream = TcpStream::connect(bound).await.expect("connect");
    let over = tokio_tungstenite::client_async(upgrade(bound, &host, &origin), stream).await;
    match over.expect_err("every place to greet from is taken") {
        tokio_tungstenite::tungstenite::Error::Http(response) => {
            assert_eq!(response.status().as_u16(), 503);
        }
        other => panic!("expected 503, got {other}"),
    }
    // Plain requests are still answered: sockets hold no HTTP slot.
    let health = format!("GET /api/health HTTP/1.1\r\nHost: {bound}\r\nConnection: close\r\n\r\n");
    assert!(raw(bound, &health).await.starts_with("HTTP/1.1 200 OK"));

    let mut leaving = open.pop().expect("one to close");
    leave(&mut leaving).await;
    drop(leaving);
    let mut again = None;
    for _ in 0..50 {
        let stream = TcpStream::connect(bound).await.expect("connect");
        match tokio_tungstenite::client_async(upgrade(bound, &host, &origin), stream).await {
            Ok((socket, _)) => {
                again = Some(socket);
                break;
            }
            Err(_) => tokio::time::sleep(Duration::from_millis(20)).await,
        }
    }
    assert!(again.is_some(), "the freed slot is reusable");
}

#[tokio::test]
async fn removing_an_origin_closes_only_its_connections() {
    let (server, bound, _shutdown) = listening(Tuning::default()).await;
    let mut public = connect(bound, PUBLIC_HOST, PUBLIC).await;
    let mut local = connect(bound, &bound.to_string(), &format!("http://{bound}")).await;
    server.apply(settings(&[])).await.expect("apply");
    assert_eq!(close_code(&mut public).await, close::DISABLED);
    assert_eq!(server.listening(), Some(bound), "the listener stayed open");
    local.send(Message::text("still here")).await.expect("send");
    assert_eq!(text(&mut local).await, "echo still here");

    let stream = TcpStream::connect(bound).await.expect("connect");
    let removed =
        tokio_tungstenite::client_async(upgrade(bound, PUBLIC_HOST, PUBLIC), stream).await;
    assert!(removed.is_err(), "the removed origin is no longer served");
}

#[tokio::test]
async fn disabling_closes_sockets_with_4503_and_frees_the_port() {
    let (server, bound, _shutdown) = listening(Tuning::default()).await;
    let pairing = server
        .start_pairing(aether_protocol::remote::DeviceRole::Operator)
        .expect("pairing is on");
    let mut socket = connect(bound, &bound.to_string(), &format!("http://{bound}")).await;
    let closing = tokio::spawn(async move { close_code(&mut socket).await });
    server
        .apply(RemoteSettings::default())
        .await
        .expect("apply");
    assert_eq!(closing.await.expect("joined"), close::DISABLED);
    assert_eq!(server.listening(), None);
    assert!(
        TcpStream::connect(bound).await.is_err(),
        "nothing listens any more"
    );
    let cancelled = server.get_pairing(&pairing.id).expect("remembered");
    assert_eq!(
        cancelled.state,
        aether_protocol::remote::PairingState::Cancelled
    );
    assert!(
        server.start_pairing(Default::default()).is_err(),
        "no pairing while closed"
    );

    // Enabling again binds afresh, on whatever port the system gives.
    server.apply(settings(&[])).await.expect("apply");
    let reopened = server.listening().expect("open again");
    let health =
        format!("GET /api/health HTTP/1.1\r\nHost: {reopened}\r\nConnection: close\r\n\r\n");
    assert!(raw(reopened, &health).await.starts_with("HTTP/1.1 200 OK"));
}

#[tokio::test]
async fn a_stopping_daemon_closes_sockets_with_1001() {
    let (server, bound, shutdown) = listening(Tuning::default()).await;
    let mut socket = connect(bound, &bound.to_string(), &format!("http://{bound}")).await;
    let closing = tokio::spawn(async move { close_code(&mut socket).await });
    shutdown.cancel();
    server.stop().await;
    assert_eq!(closing.await.expect("joined"), close::GOING_AWAY);
    assert!(
        TcpStream::connect(bound).await.is_err(),
        "nothing listens any more"
    );
}

#[tokio::test]
async fn an_address_in_use_is_reported_and_the_settings_stand() {
    let taken = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let mut config = testing::config();
    config.address = taken.local_addr().expect("addr");
    let server = RemoteServer::new(
        config,
        Arc::new(testing::EchoHost),
        Arc::new(testing::MemoryDevices::default()),
        CancellationToken::new(),
    )
    .expect("a loopback listener is allowed");
    let server = Arc::new(server);
    server
        .apply(settings(&[PUBLIC]))
        .await
        .expect("the settings are kept");
    let status = server.status(7).expect("status");
    assert_eq!((status.revision, status.listening), (7, None));
    assert!(status.settings.enabled);
    let why = status.error.expect("why it is closed");
    assert!(
        why.contains(&taken.local_addr().expect("addr").to_string()),
        "{why}"
    );

    // The port coming free, the same settings open it.
    drop(taken);
    server.apply(settings(&[PUBLIC])).await.expect("apply");
    let status = server.status(8).expect("status");
    assert!(
        status.listening.is_some() && status.error.is_none(),
        "{status:?}"
    );
    let refused = server.apply(settings(&["http://plain.example"])).await;
    assert!(refused.is_err(), "a plaintext public origin is refused");
    assert_eq!(
        server.settings(),
        settings(&[PUBLIC]),
        "and changes nothing"
    );
}

/// A server with one enrolled device, listening with places for few.
async fn crowded(tuning: Tuning) -> (Arc<RemoteServer>, SocketAddr, Device, CancellationToken) {
    let (server, devices, shutdown) = testing::server(tuning);
    server.apply(settings(&[])).await.expect("apply");
    let bound = server.listening().expect("the listener is open");
    let key = testing::key_pair();
    let record = testing::record(&key);
    devices.put(&record);
    let device = Device {
        key,
        record,
        devices,
    };
    (server, bound, device, shutdown)
}

struct Device {
    key: aws_lc_rs::signature::EcdsaKeyPair,
    record: crate::auth::DeviceRecord,
    devices: Arc<testing::MemoryDevices>,
}

impl Device {
    /// Open a socket, answer its challenge, and return what the daemon said.
    async fn greet(&self, bound: SocketAddr) -> (WebSocketStream<TcpStream>, String) {
        let origin = format!("http://{bound}");
        let (mut socket, nonce) = challenged(bound, &bound.to_string(), &origin).await;
        let hello = testing::hello(&self.key, &self.record, &origin, &nonce);
        socket.send(Message::text(hello)).await.expect("send");
        let answer = text(&mut socket).await;
        (socket, answer)
    }
}

const SHORT_PATIENCE: Duration = Duration::from_millis(50);

#[tokio::test]
async fn sockets_that_never_greet_cannot_keep_a_device_out() {
    let tuning = Tuning {
        lobby_patience: SHORT_PATIENCE,
        ..Tuning::default()
    };
    let (server, bound, device, _shutdown) = crowded(tuning).await;
    let (host, origin) = (bound.to_string(), format!("http://{bound}"));
    let mut squatters = Vec::new();
    for _ in 0..server.config().max_connections {
        squatters.push(connect(bound, &host, &origin).await);
    }
    tokio::time::sleep(SHORT_PATIENCE * 2).await;

    let (mut socket, answer) = device.greet(bound).await;
    assert_eq!(answer, "welcome phone");
    let mut displaced = squatters.remove(0);
    assert_eq!(close_code(&mut displaced).await, close::TIMEOUT);

    // Whoever arrives next takes a squatter's place, never the device's.
    for _ in 0..server.config().max_connections * 2 {
        tokio::time::sleep(SHORT_PATIENCE * 2).await;
        squatters.push(connect(bound, &host, &origin).await);
    }
    assert_eq!(server.connections().authenticated(), 1);
    socket
        .send(Message::text("still here"))
        .await
        .expect("send");
    assert_eq!(text(&mut socket).await, "echo still here");
}

#[tokio::test]
async fn device_sessions_have_a_budget_of_their_own() {
    let (server, bound, device, _shutdown) = crowded(Tuning::default()).await;
    let mut sessions = Vec::new();
    for _ in 0..server.config().max_connections {
        let (socket, answer) = device.greet(bound).await;
        assert_eq!(answer, "welcome phone");
        sessions.push(socket);
    }
    assert_eq!(server.connections().authenticated(), 4);
    let accepted = device.devices.outcomes.lock().len();

    // Greeted sockets wait nowhere, so there is room to ask; none to stay.
    let (_refused, answer) = device.greet(bound).await;
    assert_eq!(answer, "refused every device connection is in use");
    assert_eq!(
        device.devices.outcomes.lock().len(),
        accepted,
        "a hello there was no room for is no verdict on the device"
    );

    let mut leaving = sessions.pop().expect("one to close");
    leave(&mut leaving).await;
    let mut welcomed = false;
    for _ in 0..50 {
        if device.greet(bound).await.1 == "welcome phone" {
            welcomed = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(welcomed, "the freed session is reusable");
}

#[tokio::test]
async fn connections_that_send_nothing_cannot_keep_a_request_out() {
    let tuning = Tuning {
        header_timeout: Duration::from_secs(60),
        lobby_patience: SHORT_PATIENCE,
        ..Tuning::default()
    };
    let (server, bound, _shutdown) = listening(tuning).await;
    let mut silent = Vec::new();
    for _ in 0..server.config().max_connections {
        silent.push(TcpStream::connect(bound).await.expect("connect"));
    }
    let health = format!("GET /api/health HTTP/1.1\r\nHost: {bound}\r\nConnection: close\r\n\r\n");
    for _ in 0..server.config().max_connections * 2 {
        let started = tokio::time::Instant::now();
        assert!(raw(bound, &health).await.starts_with("HTTP/1.1 200 OK"));
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "answered long before the silent connections time out"
        );
    }
    let mut displaced = silent.remove(0);
    let mut nothing = [0u8; 1];
    let ended = tokio::time::timeout(Duration::from_secs(5), displaced.read(&mut nothing)).await;
    assert!(
        matches!(ended.expect("hung up on in time"), Ok(0) | Err(_)),
        "the connection that waited longest gave up its place"
    );
}

#[tokio::test]
async fn a_full_lobby_displaces_nobody_until_a_newcomer_arrives() {
    let tuning = Tuning {
        header_timeout: Duration::from_secs(60),
        lobby_patience: SHORT_PATIENCE,
        ..Tuning::default()
    };
    let (server, bound, _shutdown) = listening(tuning).await;
    let mut slow = Vec::new();
    for _ in 0..server.config().max_connections {
        slow.push(TcpStream::connect(bound).await.expect("connect"));
    }
    tokio::time::sleep(SHORT_PATIENCE * 6).await;

    let mut nothing = [0u8; 1];
    let read = tokio::time::timeout(SHORT_PATIENCE * 2, slow[0].read(&mut nothing)).await;
    assert!(read.is_err(), "the oldest still holds its place: {read:?}");

    let health = format!("GET /api/health HTTP/1.1\r\nHost: {bound}\r\nConnection: close\r\n\r\n");
    assert!(raw(bound, &health).await.starts_with("HTTP/1.1 200 OK"));
    let ended = tokio::time::timeout(Duration::from_secs(5), slow[0].read(&mut nothing)).await;
    assert!(
        matches!(
            ended.expect("displaced once someone needed the place"),
            Ok(0) | Err(_)
        ),
        "the newcomer took the oldest place"
    );
}

/// Requests enough that the answers cannot all sit in the kernel's buffers.
const UNREAD_REQUESTS: usize = 2000;

#[tokio::test]
async fn a_peer_that_stops_reading_its_responses_is_dropped() {
    let tuning = Tuning {
        write_timeout: Duration::from_millis(100),
        ..Tuning::default()
    };
    let (_server, bound, _shutdown) = listening(tuning).await;
    let request = format!("GET /styles.css HTTP/1.1\r\nHost: {bound}\r\n\r\n");
    let mut stream = TcpStream::connect(bound).await.expect("connect");
    let asked = request.repeat(UNREAD_REQUESTS);
    stream.write_all(asked.as_bytes()).await.expect("write");
    // Long enough for the listener to fill the buffers, wait, and give up.
    tokio::time::sleep(Duration::from_secs(2)).await;

    let mut answers = Vec::new();
    let reading = stream.read_to_end(&mut answers);
    let ended = tokio::time::timeout(Duration::from_secs(20), reading).await;
    // A reset or a clean end, as the kernel has it: either way it ended.
    drop(ended.expect("the connection was closed, not left waiting"));
    let answered = String::from_utf8_lossy(&answers)
        .matches("HTTP/1.1 200 OK")
        .count();
    assert!(answered > 0, "the first answers were written");
    assert!(
        answered < UNREAD_REQUESTS,
        "{answered} answers: the listener kept writing to a peer that took nothing"
    );
}
