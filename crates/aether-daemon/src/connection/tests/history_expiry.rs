use super::*;
use aether_core::{
    event::{EventDraft, EventKind},
    id::RunId,
};
use aether_store::{Commit, SqliteStore, Store};
use futures::SinkExt as _;
use tokio_util::codec::FramedWrite;

#[tokio::test]
async fn expired_subscription_reports_the_reason_and_closes_instead_of_hanging() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("history.db");
    let store = Arc::new(SqliteStore::open(&path).unwrap());
    let run = RunId::new();
    store
        .commit(
            Commit::new().event(
                EventDraft::new(
                    Actor::System,
                    EventKind::Custom {
                        name: "old".into(),
                        payload: serde_json::json!({}),
                    },
                )
                .run(run),
            ),
        )
        .unwrap();
    let (daemon, _config) = daemon_with_store(store).await;
    let daemon = Arc::new(daemon);
    // Only this disposable fixture is expired; the production sweep is not enabled.
    let mut sql = rusqlite::Connection::open(&path).unwrap();
    let tx = sql.transaction().unwrap();
    tx.execute("INSERT INTO expired_event_spans SELECT run,COALESCE(agent_id,''),COALESCE(parent_agent_id,''),COALESCE(task_id,''),type,family,MIN(seq),MAX(seq) FROM events WHERE run=?1 GROUP BY run,agent_id,parent_agent_id,task_id,type,family", [run.to_string()]).unwrap();
    tx.execute("DELETE FROM events WHERE run=?1", [run.to_string()])
        .unwrap();
    tx.commit().unwrap();
    drop(sql);

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
    output
        .send(
            aether_protocol::encode(&ClientFrame {
                id: 1,
                command: Command::Hello(aether_protocol::Hello::new("retention-test", "test")),
            })
            .unwrap()
            .trim_end()
            .to_owned(),
        )
        .await
        .unwrap();
    let hello = tokio::time::timeout(Duration::from_secs(5), input.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(matches!(
        aether_protocol::decode::<ServerFrame>(&hello).unwrap(),
        ServerFrame::Reply {
            id: 1,
            result: Ok(Reply::Welcome(_))
        }
    ));
    output
        .send(
            aether_protocol::encode(&ClientFrame {
                id: 2,
                command: Command::Subscribe {
                    after: Some(0),
                    query: Default::default(),
                    deltas: false,
                },
            })
            .unwrap()
            .trim_end()
            .to_owned(),
        )
        .await
        .unwrap();

    let mut notice = None;
    tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(line) = input.next().await {
            match aether_protocol::decode::<ServerFrame>(&line.unwrap()).unwrap() {
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
    let notice = notice.expect("reason before EOF");
    assert_eq!(notice.code, ErrorCode::NotFound);
    assert!(notice.message.contains("history expired"));
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    daemon.runtime.shutdown().await;
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
