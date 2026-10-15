#![allow(clippy::expect_used, clippy::panic)]
use super::*;
use aether_core::event::{Actor, EventDraft, EventKind};
use aether_store::{Commit, SqliteStore, Store};
use tokio_util::sync::CancellationToken;

fn expire_history(path: &std::path::Path, retained: usize) {
    let store = SqliteStore::open(path).expect("store");
    for _ in 0..=retained {
        store
            .commit(Commit::new().event(EventDraft::new(
                Actor::System,
                EventKind::Custom {
                    name: "history-fixture".into(),
                    payload: serde_json::Value::Null,
                },
            )))
            .expect("event");
    }
    assert_eq!(store.last_seq().expect("watermark"), retained as u64 + 1);
    let mut db = rusqlite::Connection::open(path).expect("database");
    let tx = db.transaction().expect("transaction");
    tx.execute("INSERT INTO expired_event_spans SELECT COALESCE(run,''), COALESCE(agent_id,''), COALESCE(parent_agent_id,''), COALESCE(task_id,''), type, family, seq, seq FROM events WHERE seq = ?1", [(retained + 1) as i64]).expect("expiry marker");
    assert_eq!(
        tx.execute("DELETE FROM events WHERE seq = ?1", [(retained + 1) as i64])
            .expect("expire"),
        1
    );
    tx.commit().expect("commit");
}

pub(super) async fn connect_when_ready(socket: &std::path::Path) -> Client {
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            match Client::connect(socket).await {
                Ok(client) => return client,
                Err(ClientError::Io(error))
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
                    ) =>
                {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                Err(error) => panic!("connect: {error}"),
            }
        }
    })
    .await
    .expect("daemon ready")
}

async fn replay_expired_history(retained: usize) {
    let directory = tempfile::tempdir().expect("directory");
    let socket = directory.path().join("d.sock");
    let store_path = directory.path().join("store.db");
    expire_history(&store_path, retained);
    let shutdown = CancellationToken::new();
    let daemon = tokio::spawn(aether_daemon::run(
        aether_daemon::DaemonConfig {
            socket: socket.clone(),
            store_path,
            config_dir: Some(directory.path().join("config")),
            workspace_root: Some(directory.path().into()),
            max_concurrent_turns: Some(1),
            yolo: false,
            tcp: None,
            allow_remote: false,
        },
        shutdown.clone(),
    ));
    let client = connect_when_ready(&socket).await;
    let mut app = consume_expired_replay(&client, retained).await;
    let fresh = Client::connect(&socket).await.expect("fresh connection");
    assert!(
        app.reconnected(&fresh)
            .iter()
            .any(|effect| matches!(effect, Effect::Subscribe { after: None }))
    );
    let live = fresh
        .subscribe(None, subscription_query(), true)
        .await
        .expect("live subscription");
    assert!(live.last_seq() >= retained as u64);
    shutdown.cancel();
    daemon.await.expect("daemon task").expect("daemon shutdown");
}

async fn consume_expired_replay(client: &Client, retained: usize) -> App {
    let (tx, mut rx) = mpsc::unbounded_channel();
    spawn_subscription(Link::new(client.clone()), tx, Some(0));
    let mut events = 0;
    let mut app = App::new(TuiOptions {
        auto_create_root: false,
        resume_seq: Some(0),
        ..Default::default()
    });
    loop {
        let next = tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .expect("event deadline")
            .expect("event");
        let LoopEvent::AppFrom { event, .. } = next else {
            panic!("scoped event");
        };
        match event {
            AppEvent::Frame(SubscriptionFrame::Event(_)) => events += 1,
            AppEvent::HistoryExpired => {
                assert!(app.reduce(event).is_empty());
                break;
            }
            AppEvent::Disconnected { because } => panic!("expiry lost: {because:?}"),
            _ => {
                app.reduce(event);
            }
        }
    }
    assert_eq!(
        events, retained,
        "expiry can follow an accepted replay page"
    );
    assert!(client.history_expired());
    assert_eq!(app.last_seq, None);
    assert!(crate::testing::render_to(&app, 180, 30).contains("history expired"));
    app
}

#[tokio::test]
async fn real_daemon_expiry_after_subscription_ack_clears_the_cursor() {
    replay_expired_history(0).await;
}

#[tokio::test]
async fn real_daemon_mid_replay_expiry_clears_the_cursor() {
    replay_expired_history(500).await;
}
