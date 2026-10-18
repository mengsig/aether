use super::*;

use std::path::Path;

use aether_client::{Client, ReconnectingSink};
use aether_protocol::{
    ClientFrame, Command as Rpc, PROTOCOL_VERSION, Reply as ProtocolReply, ServerFrame, Welcome,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::task::JoinSet;

/// A daemon double on a unix socket that welcomes and accepts every trigger.
/// Dropping it closes every connection, as a daemon restart does.
struct FakeDaemon(JoinSet<()>);

impl FakeDaemon {
    fn start(socket: &Path) -> FakeDaemon {
        let listener = UnixListener::bind(socket).expect("bind the fake daemon");
        let mut tasks = JoinSet::new();
        tasks.spawn(async move {
            let mut connections = JoinSet::new();
            while let Ok((stream, _)) = listener.accept().await {
                connections.spawn(answer(stream));
            }
        });
        FakeDaemon(tasks)
    }

    async fn stop(mut self, socket: &Path) {
        self.0.shutdown().await;
        std::fs::remove_file(socket).expect("remove the socket file");
    }
}

async fn answer(stream: UnixStream) {
    let (read, mut write) = stream.into_split();
    let mut lines = BufReader::new(read).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let frame: ClientFrame = serde_json::from_str(&line).expect("a client frame");
        let result = match frame.command {
            Rpc::Hello(_) => ProtocolReply::Welcome(Welcome {
                protocol_version: PROTOCOL_VERSION,
                daemon_version: "fake".into(),
                instance_id: "fake".into(),
                schema_version: 1,
                pid: std::process::id(),
                features: vec![],
                daemon_protocol_version: None,
                min_protocol_version: None,
                idle_timeout_secs: None,
                device: None,
                submission_horizon: None,
            }),
            Rpc::Trigger { .. } => ProtocolReply::Ok,
            other => panic!("the fake daemon was not asked for {other:?}"),
        };
        let mut encoded = serde_json::to_string(&ServerFrame::Reply {
            id: frame.id,
            result: Ok(result),
        })
        .expect("encode");
        encoded.push('\n');
        if write.write_all(encoded.as_bytes()).await.is_err() {
            return;
        }
    }
}

async fn serving_over(socket: &Path) -> (SocketAddr, CancellationToken) {
    let client = Client::connect(socket).await.expect("connect");
    let path = socket.to_path_buf();
    let sink = Arc::new(ReconnectingSink::new(client, move || {
        let path = path.clone();
        async move { Ok(Client::connect(&path).await?) }
    }));
    let config = ServeConfig {
        address: Some("127.0.0.1:0".parse().expect("addr")),
        allow_remote: false,
        replay_path: socket.with_extension("replay.db"),
        webhooks: vec![hook()],
        subscriptions: Vec::new(),
    };
    let shutdown = CancellationToken::new();
    let (tx, rx) = tokio::sync::oneshot::channel();
    let token = shutdown.clone();
    tokio::spawn(async move {
        let _ = aether_client::serve::serve_webhooks(sink, &config, token, |bound| {
            let _ = tx.send(bound.expect("an inbound address"));
        })
        .await;
    });
    (
        rx.await.expect("the listener reported its address"),
        shutdown,
    )
}

async fn post_signed(address: SocketAddr, path: &str) -> u16 {
    let delivery = Delivery::signed(r#"{"ref":"main"}"#, SECRET);
    let mut request = reqwest::Client::new()
        .post(format!("http://{address}{path}"))
        .body(delivery.body.clone());
    for (name, value) in &delivery.headers {
        request = request.header(*name, value);
    }
    request.send().await.expect("post").status().as_u16()
}

#[tokio::test]
async fn serve_answers_503_while_the_daemon_is_down_and_recovers_when_it_returns() {
    let dir = tempfile::tempdir().expect("tempdir");
    let socket = dir.path().join("d.sock");
    let daemon = FakeDaemon::start(&socket);
    let (address, shutdown) = serving_over(&socket).await;
    assert_eq!(post_signed(address, "/hooks/ci").await, 202);

    daemon.stop(&socket).await;
    assert_eq!(post_signed(address, "/hooks/ci").await, 503);
    assert_eq!(
        post_signed(address, "/hooks/ci").await,
        503,
        "still down, answered from the backoff"
    );
    assert_eq!(post_signed(address, "/nowhere").await, 404);

    let _restarted = FakeDaemon::start(&socket);
    let recovered = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if post_signed(address, "/hooks/ci").await == 202 {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await;
    assert!(recovered.is_ok(), "serve never reconnected");
    shutdown.cancel();
}
