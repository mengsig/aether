#![allow(clippy::expect_used, clippy::panic)]
use super::*;
use crate::Transport;
use aether_protocol::{ClientFrame, Reply, ServerFrame, Welcome};
use std::ops::RangeInclusive;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt as _, AsyncRead, AsyncWrite, AsyncWriteExt as _, BufReader};

const DEADLINE: Duration = Duration::from_secs(5);

/// What a daemon of another build saw: the version each hello asked for,
/// the token it carried, and every other command that reached it.
#[derive(Default)]
struct Seen {
    hellos: Vec<(u32, Option<String>)>,
    hello_features: Vec<Vec<String>>,
    commands: Vec<String>,
}

/// One connection to a daemon serving `window`, refusing a hello outside it
/// in the words `compat::negotiate` uses and closing, as the daemon does.
async fn fake_daemon_connection<S>(stream: S, window: RangeInclusive<u32>, seen: Arc<Mutex<Seen>>)
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (read, mut write) = tokio::io::split(stream);
    let mut lines = BufReader::new(read).lines();
    while let Some(line) = lines.next_line().await.expect("read client frame") {
        let frame: ClientFrame = aether_protocol::decode(&line).expect("decode client frame");
        let (result, close) = match frame.command {
            Command::Hello(hello) => {
                let asked = hello.protocol_version;
                let mut seen = seen.lock().expect("seen");
                seen.hellos.push((asked, hello.token));
                seen.hello_features.push(hello.features);
                if window.contains(&asked) {
                    (Ok(Reply::Welcome(welcome(asked, &window))), false)
                } else {
                    let refusal = format!(
                        "client protocol {asked} is not served: this daemon speaks {} to {}",
                        window.start(),
                        window.end()
                    );
                    (Err(ErrorReply::new(ErrorCode::Incompatible, refusal)), true)
                }
            }
            other => {
                seen.lock().expect("seen").commands.push(other.name());
                (Ok(Reply::Ok), false)
            }
        };
        let reply = ServerFrame::Reply {
            id: frame.id,
            result,
        };
        let line = aether_protocol::encode(&reply).expect("encode reply");
        write.write_all(line.as_bytes()).await.expect("write reply");
        if close {
            return;
        }
    }
}

fn welcome(version: u32, window: &RangeInclusive<u32>) -> Welcome {
    Welcome {
        protocol_version: version,
        daemon_version: "older".into(),
        instance_id: "inst".into(),
        schema_version: 1,
        pid: 7,
        features: vec![],
        daemon_protocol_version: Some(*window.end()),
        min_protocol_version: Some(*window.start()),
        idle_timeout_secs: None,
        device: None,
        submission_horizon: None,
    }
}

fn unix_daemon(
    window: RangeInclusive<u32>,
) -> (tempfile::TempDir, std::path::PathBuf, Arc<Mutex<Seen>>) {
    let dir = tempfile::tempdir().expect("socket directory");
    let socket = dir.path().join("daemon.sock");
    let listener = tokio::net::UnixListener::bind(&socket).expect("listen");
    let seen = Arc::new(Mutex::new(Seen::default()));
    let recorded = seen.clone();
    tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.expect("accept");
            tokio::spawn(fake_daemon_connection(
                stream,
                window.clone(),
                recorded.clone(),
            ));
        }
    });
    (dir, socket, seen)
}

fn asked(seen: &Mutex<Seen>) -> Vec<u32> {
    let seen = seen.lock().expect("seen");
    seen.hellos.iter().map(|(version, _)| *version).collect()
}

#[tokio::test]
async fn a_newer_client_speaks_an_older_daemons_version_over_the_unix_socket() {
    let older = PROTOCOL_VERSION - 1;
    let (_dir, socket, seen) = unix_daemon(MIN_PROTOCOL_VERSION..=older);
    let client = tokio::time::timeout(DEADLINE, Client::connect(&socket))
        .await
        .expect("connect must not hang")
        .expect("a version both speak is found");
    let welcome = client.welcome().expect("welcome");
    assert_eq!(welcome.protocol_version, older);
    assert_eq!(asked(&seen), vec![PROTOCOL_VERSION, older]);
    client.request(Command::Status).await.expect("status");
    assert_eq!(seen.lock().expect("seen").commands, vec!["status"]);
}

#[tokio::test]
async fn a_newer_client_speaks_an_older_daemons_version_over_tcp() {
    let older = PROTOCOL_VERSION - 1;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listen");
    let addr = listener.local_addr().expect("address");
    let seen = Arc::new(Mutex::new(Seen::default()));
    let recorded = seen.clone();
    tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.expect("accept");
            let window = MIN_PROTOCOL_VERSION..=older;
            tokio::spawn(fake_daemon_connection(stream, window, recorded.clone()));
        }
    });
    let client = tokio::time::timeout(DEADLINE, Client::connect_tcp(addr, "secret"))
        .await
        .expect("connect must not hang")
        .expect("a version both speak is found");
    assert_eq!(client.welcome().map(|w| w.protocol_version), Some(older));
    let hellos = std::mem::take(&mut seen.lock().expect("seen").hellos);
    assert_eq!(
        hellos,
        vec![
            (PROTOCOL_VERSION, Some("secret".into())),
            (older, Some("secret".into()))
        ],
        "every attempt is a fresh connection presenting the token"
    );
}

#[tokio::test]
async fn the_daemons_advertised_window_skips_versions_it_cannot_speak() {
    let (_dir, socket, seen) = unix_daemon(MIN_PROTOCOL_VERSION..=MIN_PROTOCOL_VERSION);
    let client = Client::connect(&socket).await.expect("oldest served");
    assert_eq!(
        client.welcome().map(|w| w.protocol_version),
        Some(MIN_PROTOCOL_VERSION)
    );
    assert_eq!(asked(&seen), vec![PROTOCOL_VERSION, MIN_PROTOCOL_VERSION]);
}

#[tokio::test]
async fn a_daemon_with_no_version_in_common_is_asked_once_and_named() {
    for window in [
        PROTOCOL_VERSION + 1..=PROTOCOL_VERSION + 2,
        MIN_PROTOCOL_VERSION - 2..=MIN_PROTOCOL_VERSION - 1,
    ] {
        let (_dir, socket, seen) = unix_daemon(window.clone());
        let error = Client::connect(&socket)
            .await
            .expect_err("nothing in common");
        let ClientError::Incompatible(message) = error else {
            panic!("expected Incompatible: {error:?}");
        };
        assert_eq!(asked(&seen), vec![PROTOCOL_VERSION], "{window:?}");
        let theirs = format!("speaks {} to {}", window.start(), window.end());
        let ours = format!("protocol {MIN_PROTOCOL_VERSION} to {PROTOCOL_VERSION}");
        assert!(
            message.contains(&theirs) && message.contains(&ours),
            "{message}"
        );
    }
}

#[tokio::test]
async fn an_unworded_refusal_steps_down_one_version_at_a_time() {
    let (transport, _peer) = Transport::memory(1);
    let mut held = Some(Client::from_transport(transport));
    let mut tried = Vec::new();
    let client = newest_common(|version| {
        tried.push(version);
        let result = if version == MIN_PROTOCOL_VERSION {
            held.take().ok_or(ClientError::Disconnected)
        } else {
            Err(ClientError::Incompatible("protocol 1 vs 2".into()))
        };
        async move { result }
    })
    .await
    .expect("the oldest is accepted");
    assert!(client.welcome().is_none());
    let expected: Vec<u32> = (MIN_PROTOCOL_VERSION..=PROTOCOL_VERSION).rev().collect();
    assert_eq!(tried, expected);
}

#[tokio::test]
async fn a_failure_other_than_a_refusal_is_not_retried() {
    let mut tried = Vec::new();
    let error = newest_common(|version| {
        tried.push(version);
        async { Err(ClientError::Unauthorized("wrong token".into())) }
    })
    .await
    .expect_err("refused");
    assert!(matches!(error, ClientError::Unauthorized(_)), "{error:?}");
    assert_eq!(tried, vec![PROTOCOL_VERSION]);
}

#[test]
fn the_next_version_stays_inside_both_windows() {
    let refusal = |start: u32, end: u32| format!("this daemon speaks {start} to {end}.");
    assert_eq!(
        next_version(PROTOCOL_VERSION, &refusal(MIN_PROTOCOL_VERSION, 18)),
        Some(18)
    );
    assert_eq!(
        next_version(PROTOCOL_VERSION, &refusal(1, 99)),
        Some(PROTOCOL_VERSION - 1)
    );
    assert_eq!(
        next_version(PROTOCOL_VERSION, &refusal(PROTOCOL_VERSION + 1, 99)),
        None
    );
    assert_eq!(
        next_version(PROTOCOL_VERSION, &refusal(1, MIN_PROTOCOL_VERSION - 1)),
        None
    );
    assert_eq!(next_version(MIN_PROTOCOL_VERSION, "unworded"), None);
}

fn connected_at(version: u32, features: Vec<String>) -> (Client, crate::MemoryPeer) {
    let (transport, peer) = Transport::memory(4);
    let mut client = Client::from_transport(transport);
    let mut welcome = welcome(version, &(MIN_PROTOCOL_VERSION..=version));
    welcome.features = features;
    client.welcome = Some(welcome);
    (client, peer)
}

#[tokio::test]
async fn a_command_newer_than_the_negotiated_version_is_refused_before_sending() {
    let (client, mut peer) = connected_at(17, vec![]);
    let refused = client
        .request(Command::GetAttention)
        .await
        .expect_err("refused client-side");
    let ClientError::Protocol(error) = &refused else {
        panic!("a protocol refusal: {refused:?}");
    };
    assert_eq!(error.code, ErrorCode::Unimplemented);
    assert!(
        error.message.contains("`get_attention` needs protocol 18")
            && error.message.contains("speaks protocol 17")
            && error.message.contains("Nothing was sent"),
        "{}",
        error.message
    );
    assert!(
        peer.incoming.try_recv().is_err(),
        "nothing reached the daemon"
    );
}

#[tokio::test]
async fn a_command_added_within_a_version_needs_its_feature() {
    let (client, mut peer) = connected_at(PROTOCOL_VERSION, vec![]);
    let refused = client
        .request(Command::StorageStatus)
        .await
        .expect_err("refused client-side");
    assert!(refused.to_string().contains("`storage`"), "{refused}");
    assert!(
        peer.incoming.try_recv().is_err(),
        "nothing reached the daemon"
    );

    let storage = vec![aether_protocol::features::STORAGE.to_owned()];
    let (client, _peer) = connected_at(MIN_PROTOCOL_VERSION, storage);
    assert!(client.check_introduced(&Command::StorageStatus).is_ok());
    assert!(client.check_introduced(&Command::Status).is_ok());
    assert!(client.check_introduced(&Command::GetAttention).is_err());
}

#[tokio::test]
async fn the_hello_asks_for_expired_history_with_its_span() {
    let (_dir, socket, seen) = unix_daemon(MIN_PROTOCOL_VERSION..=PROTOCOL_VERSION);
    let _client = tokio::time::timeout(DEADLINE, Client::connect(&socket))
        .await
        .expect("connect must not hang")
        .expect("connected");
    let sent = std::mem::take(&mut seen.lock().expect("seen").hello_features);
    assert_eq!(sent.len(), 1, "one hello: {sent:?}");
    assert!(
        sent[0]
            .iter()
            .any(|feature| feature == aether_protocol::features::HISTORY_EXPIRED),
        "{sent:?}"
    );
}

#[tokio::test]
async fn an_expired_history_notice_exposes_its_code_and_span() {
    let (client, peer) = connected_at(PROTOCOL_VERSION, vec![]);
    let notice = ErrorReply::history_expired(4, 9, "event history expired");
    let refusal = ClientError::Protocol(notice.clone());
    assert_eq!(refusal.code(), Some(ErrorCode::HistoryExpired));
    assert_eq!(refusal.expired_span(), Some(4..=9));
    assert_eq!(ClientError::Disconnected.expired_span(), None);

    peer.outgoing
        .send(ServerFrame::Reply {
            id: 0,
            result: Err(notice),
        })
        .await
        .expect("deliver notice");
    drop(peer);
    tokio::time::timeout(DEADLINE, async {
        while client.is_connected() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the reader sees the close");
    assert!(client.history_expired());
    assert_eq!(client.expired_span(), Some(4..=9));
}
