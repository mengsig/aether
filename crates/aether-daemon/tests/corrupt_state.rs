#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

mod support;

use aether_core::agent::{Agent, AgentStatus};
use aether_core::event::{Actor, EventDraft, EventKind};
use aether_core::id::{ArtifactId, CheckpointId};
use aether_core::time::now;
use aether_store::{Commit, Mutation, SqliteStore, Store};
use serde_json::{Value, json};
use support::{RawConn, TestDaemon};

async fn connect(daemon: &TestDaemon) -> RawConn {
    let mut connection = RawConn::connect(&daemon.socket).await.unwrap();
    connection
        .hello(aether_protocol::PROTOCOL_VERSION)
        .await
        .unwrap();
    connection
}

async fn create(connection: &mut RawConn) -> Agent {
    let reply = connection
        .call(
            json!({"cmd":"create_agent", "role":"recovery-test", "start":false,
        "model":{"provider":"claude-code", "model":"haiku"}}),
        )
        .await;
    serde_json::from_value(reply["result"]["Ok"]["data"].clone()).expect("created agent")
}

fn seed(daemon: &TestDaemon, mut bad: Agent, mut good: Agent, kind: u8) {
    let store = SqliteStore::open(daemon.store_path()).unwrap();
    good.paused = true;
    bad.paused = true;
    bad.status = AgentStatus::Running;
    if kind == 1 {
        bad.scope.workspace_root = Some(daemon.store_path().with_file_name("missing-workspace"));
    }
    let mut commit = Commit::new()
        .mutate(Mutation::UpsertAgent(bad.clone()))
        .mutate(Mutation::UpsertAgent(good));
    commit.push_event(
        EventDraft::new(Actor::System, EventKind::AgentStarted { turn: 1 })
            .agent(bad.id)
            .run(bad.run),
    );
    commit.push_event(
        EventDraft::new(
            Actor::System,
            EventKind::Custom {
                name: "bad-test".into(),
                payload: Value::Null,
            },
        )
        .agent(bad.id)
        .run(bad.run),
    );
    if kind == 2 {
        let checkpoint = serde_json::from_value(json!({
            "id":CheckpointId::new(), "agent_id":bad.id, "version":1,
            "draft":{"objective":"continue", "state":"result saved", "artifacts":[{"reference":ArtifactId::new(),"note":"required"}]},
            "provenance":{"from_seq":0,"to_seq":1}, "created_at":now()
        })).unwrap();
        commit.push_mutation(Mutation::PutCheckpoint(checkpoint));
    }
    store.commit(commit).unwrap();
    let conn = rusqlite::Connection::open(daemon.store_path()).unwrap();
    match kind {
        0 => {
            assert_eq!(conn.execute("UPDATE events SET body=json_set(body,'$.kind.type','future.unknown') WHERE agent_id=?1 AND type='custom'", [bad.id.to_string()]).unwrap(), 1);
        }
        4 => {
            assert_eq!(
                conn.execute(
                    "UPDATE events SET body='{' WHERE agent_id=?1 AND type='custom'",
                    [bad.id.to_string()]
                )
                .unwrap(),
                1
            );
        }
        3 => {
            assert_eq!(
                conn.execute(
                    "UPDATE agents SET body='{' WHERE id=?1",
                    [bad.id.to_string()]
                )
                .unwrap(),
                1
            );
        }
        _ => {}
    }
}

#[tokio::test]
async fn corrupt_record_or_missing_reference_does_not_prevent_serving_other_agents() {
    for kind in 0..5 {
        let daemon = TestDaemon::start().await;
        let mut connection = connect(&daemon).await;
        let bad = create(&mut connection).await;
        let good = create(&mut connection).await;
        seed(&daemon, bad, good.clone(), kind);
        drop(connection);
        let daemon = daemon.restart().await;
        let mut connection = connect(&daemon).await;
        let reply = connection
            .call(json!({"cmd":"list_agents", "query":{}}))
            .await;
        let agents = reply["result"]["Ok"]["data"]
            .as_array()
            .expect("healthy agent list");
        assert!(
            agents
                .iter()
                .any(|agent| agent["id"] == good.id.to_string())
        );
        let status = connection.call(json!({"cmd":"status"})).await;
        assert!(status["result"]["Ok"].is_object(), "{status}");
        let ping = connection.call(json!({"cmd":"ping"})).await;
        assert!(ping["result"]["Ok"].is_object(), "{ping}");
        daemon.stop().await;
    }
}
