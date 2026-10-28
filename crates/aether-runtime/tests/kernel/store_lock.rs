//! Another process holding the database lock past SQLite's busy timeout
//! while a turn is recording the model's reply.
use super::*;

/// Past the store's five-second busy timeout, so the first write is refused.
const LOCK_HELD: Duration = Duration::from_secs(7);

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reply_that_arrives_under_a_held_lock_is_kept_once_it_clears() {
    let state = tempfile::tempdir().expect("state dir");
    let database = state.path().join("aether.db");
    let h = harness_full(
        vec![ScriptedClient::text("LOCKTEST")],
        |_| {},
        |_| {},
        None,
        None,
        Some(Arc::new(SqliteStore::open(&database).expect("store"))),
        |_| {},
    )
    .await;
    let release = h.client.hold_next_response();
    // Long-lived, so the one reply ends in a wait instead of another turn.
    let mut chat = spec("reply LOCKTEST");
    chat.config.as_mut().expect("config").long_lived = true;
    let (agent, _) = h.runtime.create_agent(chat, Actor::user()).expect("agent");
    h.wait_for("the model to be asked", || h.client.requests().len() == 1)
        .await;

    let holder = rusqlite::Connection::open(&database).expect("second connection");
    holder
        .execute_batch("BEGIN EXCLUSIVE; UPDATE meta SET value = value WHERE key = 'last_seq';")
        .expect("hold the lock");
    release.cancel();
    tokio::time::sleep(LOCK_HELD).await;
    let still_running = h.store.agent(agent.id).expect("read").expect("agent");
    assert_eq!(
        still_running.status.kind(),
        AgentStatusKind::Running,
        "the turn gave up under the lock: {:?}",
        still_running.status
    );
    holder.execute_batch("COMMIT;").expect("release the lock");

    h.wait_until(agent.id, |s| s.kind() == AgentStatusKind::Waiting)
        .await;
    let replies: Vec<String> = h
        .store
        .events(0, 10_000, &EventFilter::for_agent(agent.id))
        .expect("events")
        .into_iter()
        .filter_map(|e| match e.kind {
            EventKind::AgentMessage {
                role: aether_core::model::MessageRole::Assistant,
                content,
                ..
            } => Some(format!("{content:?}")),
            _ => None,
        })
        .collect();
    assert_eq!(replies.len(), 1, "{replies:?}");
    assert!(replies[0].contains("LOCKTEST"), "{replies:?}");
    let invocations = h
        .store
        .invocations_for_agent(agent.id)
        .expect("invocations");
    assert_eq!(invocations.len(), 1, "{invocations:?}");
    assert!(invocations[0].usage.output_tokens > 0, "{invocations:?}");
    let agent = h.store.agent(agent.id).expect("read").expect("agent");
    assert_eq!(agent.stats.turns, 1);
    assert_eq!(h.client.requests().len(), 1, "the turn was not run again");
}
