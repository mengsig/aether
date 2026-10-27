use super::*;
use aether_core::{
    agent::{Agent, AgentConfig, AgentStats, AgentStatus, Role, Scope},
    event::{EventDraft, EventKind},
    id::{AgentId, RunId},
    model::ModelRef,
    time::now,
};
use aether_store::{Commit, Mutation, RunExpiry, SqliteStore, Store};
use futures::SinkExt as _;
use tokio_util::codec::FramedWrite;

/// An agent that finished long ago in `run`, so retention may expire the run.
fn finished_agent(run: RunId) -> Agent {
    let at = now() - chrono::Duration::days(14);
    Agent {
        id: AgentId::new(),
        parent: None,
        role: Role::new("worker"),
        name: "finished".into(),
        scope: Scope::default(),
        model: ModelRef::new("mock", "test"),
        fallbacks: vec![],
        status: AgentStatus::Completed { summary: None },
        current_task: None,
        inbox_cursor: 0,
        checkpoint: None,
        engine_session: None,
        engine_usage: None,
        engine_sent: None,
        status_since: None,
        archived: false,
        paused: false,
        paused_via: None,
        resume_wait: None,
        restarted_from: None,
        restarted_as: None,
        engine_alive: false,
        last_event_at: None,
        config: AgentConfig::default(),
        stats: AgentStats::default(),
        run,
        created_at: at,
        updated_at: at,
    }
}

/// A daemon whose first run (seqs 1 and 2) retention has expired; seq 3 is kept.
async fn daemon_with_expired_run() -> (Arc<Daemon>, tempfile::TempDir, tempfile::TempDir) {
    let directory = tempfile::tempdir().unwrap();
    let store = Arc::new(SqliteStore::open(directory.path().join("history.db")).unwrap());
    let (expired, retained) = (RunId::new(), RunId::new());
    store
        .commit(Commit::new().mutate(Mutation::UpsertAgent(finished_agent(expired))))
        .unwrap();
    for run in [expired, expired, retained] {
        let event = EventDraft::new(
            Actor::System,
            EventKind::Custom {
                name: "old".into(),
                payload: serde_json::json!({}),
            },
        );
        store.commit(Commit::new().event(event.run(run))).unwrap();
    }
    let (daemon, config) = daemon_with_store(store).await;
    // Retention 0: everything finished before now is old enough. Events are
    // judged to the second, so the cutoff sits just past this one.
    let cutoff = now() + chrono::Duration::seconds(2);
    let expiry = daemon.runtime.expire_run_history(expired, cutoff).unwrap();
    assert!(
        matches!(expiry, RunExpiry::Expired { events: 2, .. }),
        "the expired run held two events: {expiry:?}"
    );
    (Arc::new(daemon), config, directory)
}

/// Greet with `features`, subscribe from the start, and return the welcome
/// and the notice the daemon closed the connection with.
async fn subscribe_into_expired_history(features: Vec<String>) -> (Welcome, ErrorReply) {
    let (daemon, _config, _directory) = daemon_with_expired_run().await;
    let (client, server) = tokio::io::duplex(16 * 1024);
    let task = tokio::spawn(serve(
        daemon.clone(),
        server,
        ConnectionPolicy::default(),
        None,
        None,
        CancellationToken::new(),
    ));
    let (read, write) = tokio::io::split(client);
    let mut input = FramedRead::new(read, LinesCodec::new());
    let mut output = FramedWrite::new(write, LinesCodec::new());
    let mut hello = aether_protocol::Hello::new("retention-test", "test");
    hello.features = features;
    let subscribe = Command::Subscribe {
        after: Some(0),
        query: Default::default(),
        deltas: false,
    };
    for (id, command) in [(1, Command::Hello(hello)), (2, subscribe)] {
        let line = aether_protocol::encode(&ClientFrame { id, command }).unwrap();
        output.send(line.trim_end().to_owned()).await.unwrap();
    }

    let (mut welcome, mut notice) = (None, None);
    tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(line) = input.next().await {
            match aether_protocol::decode::<ServerFrame>(&line.unwrap()).unwrap() {
                ServerFrame::Reply {
                    id: 1,
                    result: Ok(Reply::Welcome(greeting)),
                } => welcome = Some(greeting),
                ServerFrame::Reply {
                    id: 0,
                    result: Err(error),
                } => notice = Some(error),
                ServerFrame::Reply {
                    id: 2,
                    result: Ok(Reply::Subscribed { .. }),
                } => {}
                other => panic!("unexpected frame: {other:?}"),
            }
        }
    })
    .await
    .expect("failed replay must close the stream");
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    daemon.runtime.shutdown().await;
    (
        welcome.expect("welcome"),
        notice.expect("reason before EOF"),
    )
}

#[tokio::test]
async fn expired_subscription_reports_the_span_to_a_client_with_the_feature() {
    let feature = aether_protocol::features::HISTORY_EXPIRED.to_owned();
    let (welcome, notice) = subscribe_into_expired_history(vec![feature.clone()]).await;
    assert!(welcome.features.contains(&feature), "{welcome:?}");
    assert_eq!(notice.code, ErrorCode::HistoryExpired, "{notice:?}");
    assert_eq!((notice.first_seq, notice.last_seq), (Some(1), Some(2)));
    assert_eq!(notice.expired_span(), Some(1..=2));
    assert!(notice.message.contains("history expired"));
}

/// Older hosts (and any client that does not list the feature) keep the
/// `NotFound` they always got, word for word, and the connection still closes.
#[tokio::test]
async fn expired_subscription_reports_the_reason_and_closes_instead_of_hanging() {
    let (welcome, notice) =
        subscribe_into_expired_history(aether_protocol::Hello::new("x", "y").features).await;
    assert!(
        welcome
            .features
            .iter()
            .any(|feature| feature == aether_protocol::features::HISTORY_EXPIRED)
    );
    assert_eq!(
        notice,
        ErrorReply::new(
            ErrorCode::NotFound,
            "event history expired within sequence range 1..=2; obtain a fresh state snapshot before resuming replay",
        )
    );
}

#[tokio::test]
async fn subscription_rejects_a_history_before_cursor() {
    let (daemon, _config) = daemon().await;
    let daemon = Arc::new(daemon);
    let (client, server) = tokio::io::duplex(16 * 1024);
    let cancel = CancellationToken::new();
    let task = tokio::spawn(serve(
        daemon.clone(),
        server,
        ConnectionPolicy::default(),
        None,
        None,
        cancel.clone(),
    ));
    let (read, write) = tokio::io::split(client);
    let mut input = FramedRead::new(read, LinesCodec::new());
    let mut output = FramedWrite::new(write, LinesCodec::new());
    for (id, command) in [
        (
            1,
            Command::Hello(aether_protocol::Hello::new("history-test", "test")),
        ),
        (
            2,
            Command::Subscribe {
                after: None,
                deltas: false,
                query: aether_protocol::EventQuery {
                    before: Some(42),
                    ..Default::default()
                },
            },
        ),
    ] {
        output
            .send(
                aether_protocol::encode(&ClientFrame { id, command })
                    .unwrap()
                    .trim_end()
                    .to_owned(),
            )
            .await
            .unwrap();
        let line = tokio::time::timeout(Duration::from_secs(5), input.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let frame: ServerFrame = aether_protocol::decode(&line).unwrap();
        if id == 2 {
            assert!(matches!(
                frame,
                ServerFrame::Reply {
                    id: 2,
                    result: Err(ErrorReply {
                        code: ErrorCode::InvalidArgument,
                        ..
                    })
                }
            ));
        } else {
            assert!(matches!(
                frame,
                ServerFrame::Reply {
                    result: Ok(Reply::Welcome(_)),
                    ..
                }
            ));
        }
    }
    cancel.cancel();
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    daemon.runtime.shutdown().await;
}
