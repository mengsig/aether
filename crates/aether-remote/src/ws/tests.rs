use std::sync::Arc;
use std::time::Duration;

use aether_protocol::{ErrorCode, ServerFrame};
use aws_lc_rs::signature::EcdsaKeyPair;
use futures::{SinkExt as _, StreamExt as _};
use hyper::header::HeaderValue;
use tokio::io::DuplexStream;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;

use super::*;
use crate::auth::DeviceRecord;
use crate::testing::{self, FLOOD_LINES, MemoryDevices};
use crate::{RemoteServer, Tuning};

const ORIGIN: &str = "https://host.example";
/// Room between the two ends; small enough that a flood fills it.
const PIPE_BYTES: usize = 8 * 1024;

/// One carried connection, with the client's end of it.
struct Carried {
    client: WebSocketStream<DuplexStream>,
    server: Arc<RemoteServer>,
    devices: Arc<MemoryDevices>,
    /// The daemon's own shutdown.
    shutdown: CancellationToken,
    /// The listener's stop: a child of `shutdown`.
    stop: CancellationToken,
    carrying: JoinHandle<()>,
    nonce: String,
}

async fn carried(tuning: Tuning) -> Carried {
    let (server, devices, shutdown) = testing::server(tuning);
    let stop = shutdown.child_token();
    let (ours, theirs) = tokio::io::duplex(PIPE_BYTES);
    let seat = server.greeting_seat().expect("room to greet");
    let carrying = tokio::spawn(carry(theirs, server.link(ORIGIN.into(), seat, &stop)));
    let mut client = WebSocketStream::from_raw_socket(ours, Role::Client, None).await;
    let challenge = text(&mut client).await;
    let nonce = challenge
        .strip_prefix("challenge ")
        .expect("a challenge comes first");
    Carried {
        nonce: nonce.to_owned(),
        client,
        server,
        devices,
        shutdown,
        stop,
        carrying,
    }
}

impl Carried {
    /// Enrol a device and greet as it.
    async fn greet(&mut self) -> (EcdsaKeyPair, DeviceRecord) {
        let key = testing::key_pair();
        let record = testing::record(&key);
        self.devices.put(&record);
        let hello = testing::hello(&key, &record, ORIGIN, &self.nonce);
        self.say(&hello).await;
        assert_eq!(text(&mut self.client).await, "welcome phone");
        (key, record)
    }

    async fn say(&mut self, line: &str) {
        self.client.send(Message::text(line)).await.expect("send");
    }

    /// Read to the close frame: the texts before it, its code, and the
    /// carrier finishing once this end hangs up.
    async fn closed(mut self) -> (Vec<String>, u16) {
        let mut texts = Vec::new();
        let code = loop {
            let next = tokio::time::timeout(Duration::from_secs(10), self.client.next()).await;
            match next
                .expect("a close in time")
                .expect("a close before the end")
            {
                Ok(Message::Text(text)) => texts.push(text.to_string()),
                Ok(Message::Close(frame)) => break u16::from(frame.expect("a close code").code),
                Ok(_) => {}
                Err(error) => panic!("the connection failed before closing: {error}"),
            }
        };
        drop(self.client);
        self.carrying.await.expect("the carrier finishes");
        (texts, code)
    }
}

/// The next text message, keepalives aside.
async fn text(client: &mut WebSocketStream<DuplexStream>) -> String {
    loop {
        let next = tokio::time::timeout(Duration::from_secs(10), client.next()).await;
        match next
            .expect("a message in time")
            .expect("a message")
            .expect("a frame")
        {
            Message::Text(text) => return text.to_string(),
            Message::Ping(_) | Message::Pong(_) => {}
            other => panic!("expected text, got {other:?}"),
        }
    }
}

/// The error code of a closing notice: a reply frame with id 0.
fn notice(texts: &[String]) -> ErrorCode {
    let last = texts.last().expect("a notice before the close");
    match serde_json::from_str(last).expect("a server frame") {
        ServerFrame::Reply {
            id: 0,
            result: Err(error),
        } => error.code,
        other => panic!("expected an id-0 error reply, got {other:?}"),
    }
}

fn upgrade_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(CONNECTION, HeaderValue::from_static("keep-alive, Upgrade"));
    headers.insert(UPGRADE, HeaderValue::from_static("websocket"));
    headers.insert(SEC_WEBSOCKET_VERSION, HeaderValue::from_static("13"));
    headers.insert(
        SEC_WEBSOCKET_KEY,
        HeaderValue::from_static("dGhlIHNhbXBsZSBub25jZQ=="),
    );
    headers.insert(
        SEC_WEBSOCKET_PROTOCOL,
        HeaderValue::from_static("chat, aether"),
    );
    headers
}

#[test]
fn the_accept_key_matches_the_rfc_example() {
    assert_eq!(
        accept_key(b"dGhlIHNhbXBsZSBub25jZQ=="),
        "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
    );
    assert_eq!(
        accept(&upgrade_headers()).as_deref(),
        Ok("s3pPLMBiTxaQ9kYGzzhZRbK+xOo=")
    );
}

#[test]
fn each_ending_has_the_documented_close_code() {
    assert_eq!(close::for_ended(Ended::Closed), 1000);
    assert_eq!(close::for_ended(Ended::ShuttingDown), 1001);
    assert_eq!(close::for_ended(Ended::Refused), 4401);
    assert_eq!(close::for_ended(Ended::Revoked), 4403);
    assert_eq!(close::for_ended(Ended::TimedOut), 4408);
    assert_eq!(
        (close::UNSUPPORTED_DATA, close::TOO_BIG, close::DISABLED),
        (1003, 1009, 4503)
    );
}

#[test]
fn an_upgrade_must_be_a_version_13_websocket_offering_the_subprotocol() {
    let without = |name: HeaderName| {
        let mut headers = upgrade_headers();
        headers.remove(name);
        headers
    };
    let with = |name: HeaderName, value: &'static str| {
        let mut headers = upgrade_headers();
        headers.insert(name, HeaderValue::from_static(value));
        headers
    };
    let mut two_keys = upgrade_headers();
    two_keys.append(
        SEC_WEBSOCKET_KEY,
        HeaderValue::from_static("dGhlIHNhbXBsZSBub25jZQ=="),
    );
    for (refused, why) in [
        (without(UPGRADE), "no upgrade"),
        (without(CONNECTION), "no connection token"),
        (with(UPGRADE, "h2c"), "another protocol"),
        (with(SEC_WEBSOCKET_VERSION, "8"), "an old version"),
        (without(SEC_WEBSOCKET_KEY), "no key"),
        (with(SEC_WEBSOCKET_KEY, "c2hvcnQ="), "a short key"),
        (
            with(SEC_WEBSOCKET_KEY, "not base64 at all!!!!!!!"),
            "a key that is not base64",
        ),
        (two_keys, "two keys"),
        (without(SEC_WEBSOCKET_PROTOCOL), "no subprotocol"),
        (with(SEC_WEBSOCKET_PROTOCOL, "chat"), "another subprotocol"),
        (
            with(SEC_WEBSOCKET_PROTOCOL, "Aether"),
            "the subprotocol in the wrong case",
        ),
    ] {
        assert!(accept(&refused).is_err(), "{why} must be refused");
    }
}

#[tokio::test]
async fn one_text_message_is_one_line_each_way() {
    let mut carried = carried(Tuning::default()).await;
    carried.say("first").await;
    assert_eq!(text(&mut carried.client).await, "echo first");
    // A trailing newline is the line's own terminator, not a second line.
    carried.say("second\n").await;
    carried.say("third").await;
    assert_eq!(text(&mut carried.client).await, "echo second");
    assert_eq!(text(&mut carried.client).await, "echo third");
    let leaving = CloseFrame {
        code: close::NORMAL.into(),
        reason: "".into(),
    };
    carried.client.close(Some(leaving)).await.expect("close");
    let (texts, code) = carried.closed().await;
    assert_eq!((texts, code), (vec![], close::NORMAL));
}

#[tokio::test]
async fn a_binary_message_closes_with_1003() {
    let mut carried = carried(Tuning::default()).await;
    let binary = Message::binary(b"{}".to_vec());
    carried.client.send(binary).await.expect("send");
    let (texts, code) = carried.closed().await;
    assert_eq!(code, close::UNSUPPORTED_DATA);
    assert_eq!(notice(&texts), ErrorCode::InvalidArgument);
}

#[tokio::test]
async fn a_message_holding_two_lines_closes_with_1003() {
    let mut carried = carried(Tuning::default()).await;
    carried.say("one\ntwo").await;
    let (texts, code) = carried.closed().await;
    assert_eq!(code, close::UNSUPPORTED_DATA);
    assert_eq!(notice(&texts), ErrorCode::InvalidArgument);
    assert!(
        !texts.iter().any(|t| t.starts_with("echo")),
        "nothing reached the daemon"
    );
}

#[tokio::test]
async fn an_oversized_message_before_the_hello_closes_with_1009() {
    let mut carried = carried(Tuning::default()).await;
    let oversized = "x".repeat(PRE_AUTH_MAX_MESSAGE + 1);
    let sending = carried.client.send(Message::text(oversized));
    // The carrier stops reading at the header, so the send may never finish.
    let _ = tokio::time::timeout(Duration::from_millis(200), sending).await;
    let (texts, code) = carried.closed().await;
    assert_eq!(code, close::TOO_BIG);
    assert_eq!(notice(&texts), ErrorCode::InvalidArgument);
}

#[tokio::test]
async fn the_largest_pre_hello_message_passes_and_a_greeted_device_may_send_more() {
    let mut carried = carried(Tuning::default()).await;
    let largest = "y".repeat(PRE_AUTH_MAX_MESSAGE);
    carried.say(&largest).await;
    assert_eq!(
        text(&mut carried.client).await.len(),
        "echo ".len() + PRE_AUTH_MAX_MESSAGE
    );
    carried.greet().await;
    assert_eq!(carried.server.connections().authenticated(), 1);
    let larger = "z".repeat(4 * PRE_AUTH_MAX_MESSAGE);
    carried.say(&larger).await;
    assert_eq!(
        text(&mut carried.client).await.len(),
        "echo ".len() + larger.len()
    );
}

#[tokio::test]
async fn a_wrong_signature_closes_with_4401_and_is_reported() {
    let mut carried = carried(Tuning::default()).await;
    let key = testing::key_pair();
    let record = testing::record(&key);
    carried.devices.put(&record);
    let forged = testing::hello(&testing::key_pair(), &record, ORIGIN, &carried.nonce);
    carried.say(&forged).await;
    let devices = carried.devices.clone();
    let server = carried.server.clone();
    let (texts, code) = carried.closed().await;
    assert_eq!(code, close::UNAUTHORIZED);
    assert!(texts.iter().any(|t| t.starts_with("refused ")), "{texts:?}");
    let outcomes = devices.outcomes.lock();
    assert_eq!(outcomes.len(), 1, "{outcomes:?}");
    assert!(outcomes[0].starts_with("refused "), "{outcomes:?}");
    assert_eq!(server.connections().authenticated(), 0);
}

#[tokio::test]
async fn an_answer_to_another_connections_challenge_is_refused() {
    let mut first = carried(Tuning::default()).await;
    let (key, record) = first.greet().await;
    let replayed = testing::hello(&key, &record, ORIGIN, &first.nonce);

    let mut second = carried(Tuning::default()).await;
    second.devices.put(&record);
    assert_ne!(
        second.nonce, first.nonce,
        "every connection gets its own nonce"
    );
    second.say(&replayed).await;
    let (_, code) = second.closed().await;
    assert_eq!(code, close::UNAUTHORIZED);

    // The same connection takes one hello: a second answer is spent.
    first.say(&replayed).await;
    let (texts, code) = first.closed().await;
    assert_eq!(code, close::UNAUTHORIZED);
    assert!(
        texts.iter().any(|t| t.contains("already answered")),
        "{texts:?}"
    );
}

#[tokio::test]
async fn revoking_a_device_closes_its_connection_with_4403() {
    let mut carried = carried(Tuning::default()).await;
    let (_, record) = carried.greet().await;
    assert_eq!(carried.server.connections().of(record.id), 1);
    carried.server.sessions().close(record.id);
    let server = carried.server.clone();
    let (texts, code) = carried.closed().await;
    assert_eq!(notice(&texts), ErrorCode::Forbidden);
    assert_eq!(code, close::REVOKED);
    assert_eq!(server.connections().of(record.id), 0);
}

#[tokio::test]
async fn closing_the_listener_says_4503_and_stopping_the_daemon_says_1001() {
    let disabled = carried(Tuning::default()).await;
    disabled.stop.cancel();
    let (texts, code) = disabled.closed().await;
    assert_eq!(code, close::DISABLED);
    assert_eq!(notice(&texts), ErrorCode::Unavailable);

    let stopping = carried(Tuning::default()).await;
    stopping.shutdown.cancel();
    let (texts, code) = stopping.closed().await;
    assert_eq!(code, close::GOING_AWAY);
    assert_eq!(notice(&texts), ErrorCode::Unavailable);
}

#[tokio::test]
async fn a_daemon_timeout_is_4408_and_a_daemon_fault_is_1011() {
    let mut idle = carried(Tuning::default()).await;
    idle.say("idle").await;
    assert_eq!(idle.closed().await.1, close::TIMEOUT);

    let mut failing = carried(Tuning::default()).await;
    failing.say("fail").await;
    let (texts, code) = failing.closed().await;
    assert_eq!(code, close::INTERNAL);
    assert_eq!(notice(&texts), ErrorCode::Internal);
}

#[tokio::test]
async fn a_quiet_connection_is_pinged_and_a_silent_peer_is_dropped_with_4408() {
    let tuning = Tuning {
        ping_interval: Duration::from_millis(40),
        pong_timeout: Duration::from_millis(200),
        ..Tuning::default()
    };
    let mut answering = carried(tuning).await;
    // Reading answers pings, so a peer that keeps reading stays connected.
    let started = tokio::time::Instant::now();
    let mut pings = 0;
    while started.elapsed() < Duration::from_millis(500) {
        let next = tokio::time::timeout(Duration::from_secs(5), answering.client.next()).await;
        match next
            .expect("a ping in time")
            .expect("still open")
            .expect("a frame")
        {
            Message::Ping(_) => pings += 1,
            other => panic!("expected only pings, got {other:?}"),
        }
    }
    assert!(pings >= 3, "{pings} pings in half a second at 40 ms");

    let silent = carried(tuning).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let (texts, code) = silent.closed().await;
    assert_eq!(code, close::TIMEOUT);
    assert_eq!(notice(&texts), ErrorCode::Unavailable);
}

#[tokio::test]
async fn a_peer_that_stops_reading_is_dropped_with_1013() {
    let tuning = Tuning {
        stall_timeout: Duration::from_millis(150),
        close_timeout: Duration::from_secs(10),
        ..Tuning::default()
    };
    let mut carried = carried(tuning).await;
    carried.say("flood").await;
    // Far more than the pipe holds, and nobody reading: the carrier stalls.
    tokio::time::sleep(Duration::from_millis(600)).await;
    let (texts, code) = carried.closed().await;
    assert_eq!(code, close::SLOW);
    assert_eq!(notice(&texts), ErrorCode::Unavailable);
    assert!(
        texts.len() < FLOOD_LINES,
        "the flood was cut short at {}",
        texts.len()
    );
}

#[tokio::test]
async fn oversized_control_header_closes_before_payload_in_both_admission_phases() {
    use tokio::io::AsyncWriteExt;
    for authenticated in [false, true] {
        let mut carried = carried(Tuning::default()).await;
        if authenticated {
            carried.greet().await;
        }
        let mut header = vec![0x89, 0xff];
        header.extend_from_slice(&(MAX_MESSAGE as u64).to_be_bytes());
        header.extend_from_slice(&[1, 2, 3, 4]);
        assert_eq!(header.len(), 14);
        carried.client.get_mut().write_all(&header).await.unwrap();
        let (texts, code) = carried.closed().await;
        assert!(
            texts.is_empty(),
            "invalid control frames never reach the daemon"
        );
        assert_eq!(code, close::PROTOCOL);
    }
}

#[tokio::test]
async fn revocation_discards_a_stalled_large_response_and_ends_the_carrier() {
    use tokio::io::AsyncReadExt;
    let mut carried = carried(Tuning::default()).await;
    let (_, record) = carried.greet().await;
    let response_bytes = 1024 * 1024;
    carried.say(&"x".repeat(response_bytes)).await;
    let mut first = [0; 1];
    carried
        .client
        .get_mut()
        .read_exact(&mut first)
        .await
        .unwrap();
    assert_eq!(
        first[0], 0x81,
        "the pending response started its text frame"
    );
    carried.server.sessions().close(record.id);
    tokio::time::timeout(Duration::from_secs(2), carried.carrying)
        .await
        .expect("revocation ends even without reader progress")
        .unwrap();
    let mut remaining = Vec::new();
    carried
        .client
        .get_mut()
        .read_to_end(&mut remaining)
        .await
        .unwrap();
    assert!(
        remaining.len() <= PIPE_BYTES,
        "only bytes already in the transport survive"
    );
    assert!(
        remaining.len() < response_bytes,
        "the accepted snapshot was discarded"
    );
    assert_eq!(carried.server.connections().of(record.id), 0);
}
