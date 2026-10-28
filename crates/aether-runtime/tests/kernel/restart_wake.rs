//! restart wake scenarios and fixtures.
use super::*;

/// VISION §3.8: a failed agent comes back from its last checkpoint, with its
/// transcript tail, as a new agent linked to the old one in both directions.
#[tokio::test]
async fn a_failed_agent_restarts_from_its_checkpoint() {
    use aether_core::checkpoint::{Checkpoint, CheckpointDraft, CheckpointProvenance};
    use aether_core::id::CheckpointId;

    let h = harness(vec![
        ScriptedClient::tool_call("fail_task", json!({"error": "the index is corrupt"})),
        ScriptedClient::text("Picking up where the last one stopped."),
    ])
    .await;
    let (old, task) = h
        .runtime
        .create_agent(spec("index the repository"), Actor::user())
        .expect("create");
    let task = task.expect("task");
    h.wait_until(old.id, |s| s.kind() == AgentStatusKind::Failed)
        .await;

    // What a compaction would have left behind before the failure.
    let source = Checkpoint {
        id: CheckpointId::new(),
        agent_id: old.id,
        version: 3,
        draft: CheckpointDraft {
            objective: "index the repository".into(),
            state: "half the files are indexed".into(),
            open_loops: vec!["the corrupt shard needs rebuilding".into()],
            ..Default::default()
        },
        provenance: CheckpointProvenance {
            from_seq: 0,
            to_seq: 0,
            source_snapshot: None,
            compactor_model: None,
            previous: None,
            trigger: None,
        },
        created_at: now(),
    };
    h.store
        .commit(Commit::new().mutate(Mutation::PutCheckpoint(source.clone())))
        .expect("seed checkpoint");
    let carried = h.store.transcript(old.id, 0).expect("transcript");
    assert!(
        !carried.is_empty(),
        "the failed agent has something to pass on"
    );

    let fresh = h
        .runtime
        .restart_agent(old.id, Actor::user())
        .expect("restart");

    // Both directions of the link.
    assert_eq!(fresh.restarted_from, Some(old.id));
    let old_after = h.store.agent(old.id).expect("agent").expect("exists");
    assert_eq!(old_after.restarted_as, Some(fresh.id));
    assert_eq!(
        old_after.status.kind(),
        AgentStatusKind::Failed,
        "the old agent is history, not rewritten"
    );
    for id in [old.id, fresh.id] {
        assert!(
            types(h.store.as_ref(), id).contains(&"agent.restarted".to_owned()),
            "agent.restarted is on both ends of the link"
        );
    }

    // The checkpoint came across as its own record, pointing back at the old.
    let inherited = h
        .store
        .latest_checkpoint(fresh.id)
        .expect("checkpoint")
        .expect("inherited");
    assert_eq!(Some(inherited.id), fresh.checkpoint);
    assert_ne!(inherited.id, source.id);
    assert_eq!(inherited.provenance.previous, Some(source.id));
    assert_eq!(inherited.draft, source.draft);

    // The transcript tail came across, in order.
    let tail = h.store.transcript(fresh.id, 0).expect("transcript");
    assert_eq!(
        tail.iter().map(|e| e.message.text()).collect::<Vec<_>>(),
        carried.iter().map(|e| e.message.text()).collect::<Vec<_>>()
    );
    assert!(
        tail.iter().all(|e| e.invocation.is_none()),
        "the new agent never made the old agent's calls"
    );

    // It has work again: the old task ended with the old agent.
    let new_task = h
        .store
        .task(fresh.current_task.expect("task"))
        .expect("task")
        .expect("exists");
    assert_ne!(new_task.id, task.id);
    assert_eq!(new_task.objective, "index the repository");
    assert_eq!(new_task.owner, Some(fresh.id));
    assert_eq!(fresh.status.kind(), AgentStatusKind::Ready);
    assert_eq!(
        fresh.stats.usage.total_tokens(),
        0,
        "spend stays where it was spent"
    );

    // A restart happens once: a second one would fork the history.
    let err = h
        .runtime
        .restart_agent(old.id, Actor::user())
        .expect_err("already restarted");
    assert!(err.to_string().contains(&fresh.id.to_string()), "{err}");
    let err = h
        .runtime
        .restart_agent(fresh.id, Actor::user())
        .expect_err("live agent");
    assert!(err.to_string().contains("ready"), "{err}");

    // And it works from what it inherited: the next request carries both.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while h.client.remaining() > 0 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the restarted agent never took a turn"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let request = h.client.requests().pop().expect("request");
    let system: String = request.system.iter().map(|b| b.text.clone()).collect();
    assert!(
        system.contains("half the files are indexed"),
        "the inherited checkpoint is in the context: {system}"
    );
    assert!(
        request
            .messages
            .iter()
            .any(|m| m.text().contains("index the repository")),
        "the carried tail is in the context"
    );
}

/// A failed agent is woken in place: the same id, its transcript intact, its
/// failure still on the record, and reachable by message again. Before the
/// wake a message is refused, as it always was; the wake is what a person
/// does about it.
#[tokio::test]
async fn a_failed_agent_is_woken_in_place_and_answers_again() {
    let h = harness(vec![
        ScriptedClient::tool_call("fail_task", json!({"error": "the index is corrupt"})),
        ScriptedClient::text("Back; the index is still corrupt. Waiting for instructions."),
        ScriptedClient::text("Rebuilding the index now."),
    ])
    .await;
    let (agent, task) = h
        .runtime
        .create_agent(spec("index the repository"), Actor::user())
        .expect("create");
    let task = task.expect("task");
    h.wait_until(agent.id, |s| s.kind() == AgentStatusKind::Failed)
        .await;
    let err = h
        .runtime
        .send_message(agent.id, vec![ContentBlock::text("hello?")], Actor::user())
        .expect_err("a failed agent is not reachable");
    assert!(err.to_string().contains("failed"), "{err}");
    let before = h.store.transcript(agent.id, 0).expect("transcript");
    assert!(!before.is_empty(), "the failure left a history to keep");

    let woken = h.runtime.wake_agent(agent.id, Actor::user()).expect("wake");
    assert_eq!(woken.id, agent.id, "the same agent, not a successor");
    assert_eq!(woken.status.kind(), AgentStatusKind::Ready);
    assert_eq!(woken.restarted_as, None);
    assert_eq!(
        woken.current_task, None,
        "the task that ended with the failure is not carried as current"
    );
    let ended = h.store.task(task.id).expect("task").expect("exists");
    assert_eq!(
        ended.status.kind(),
        TaskStatusKind::Failed,
        "the failure record stands"
    );
    let t = types(h.store.as_ref(), agent.id);
    let failed_at = t
        .iter()
        .position(|e| e == "agent.failed")
        .expect("agent.failed");
    let resumed_at = t
        .iter()
        .rposition(|e| e == "agent.resumed")
        .expect("agent.resumed");
    assert!(
        failed_at < resumed_at,
        "the wake follows the failure: {t:?}"
    );
    // Waking it again is refused: it is no longer failed.
    let err = h
        .runtime
        .wake_agent(agent.id, Actor::user())
        .expect_err("only a failed agent is woken");
    assert!(err.to_string().contains("only a failed agent"), "{err}");

    // The wake item gives it a turn, after which it parks like any chat agent.
    h.wait_for("the woken agent took its wake turn", || {
        h.client.remaining() == 1
    })
    .await;
    h.wait_until(agent.id, |s| s.kind() == AgentStatusKind::Waiting)
        .await;

    // Reachable again, and it acts on the message with its history intact.
    h.runtime
        .send_message(
            agent.id,
            vec![ContentBlock::text("rebuild the index")],
            Actor::user(),
        )
        .expect("a woken agent is reachable");
    h.wait_for("the woken agent answered", || h.client.remaining() == 0)
        .await;
    h.wait_until(agent.id, |s| s.kind() == AgentStatusKind::Waiting)
        .await;
    let request = h.client.requests().pop().expect("request");
    let texts: Vec<String> = request.messages.iter().map(|m| m.text()).collect();
    assert!(
        texts.iter().any(|m| m.contains("rebuild the index")),
        "the new message reached it: {texts:?}"
    );
    assert!(
        texts
            .iter()
            .any(|m| m.contains("Your previous turn failed: the index is corrupt")),
        "the wake and its reason are part of the history it works from: {texts:?}"
    );
    let after = h.store.transcript(agent.id, 0).expect("transcript");
    assert!(
        after.len() > before.len(),
        "the history grew, not restarted"
    );
    let text = |entries: &[aether_store::TranscriptEntry]| -> Vec<String> {
        entries.iter().map(|e| e.message.text()).collect()
    };
    assert_eq!(
        text(&after[..before.len()]),
        text(&before),
        "everything from before the failure is still there"
    );
}

/// A wake is refused where it would only be undone, or where a restart has
/// already carried the agent on: a successor exists, or the agent is live.
#[tokio::test]
async fn a_wake_is_refused_where_a_restart_carried_the_agent_on() {
    let h = harness(vec![
        ScriptedClient::tool_call("fail_task", json!({"error": "the index is corrupt"})),
        ScriptedClient::text("Picking up where the last one stopped."),
    ])
    .await;
    let (old, _) = h
        .runtime
        .create_agent(spec("index the repository"), Actor::user())
        .expect("create");
    h.wait_until(old.id, |s| s.kind() == AgentStatusKind::Failed)
        .await;
    let fresh = h
        .runtime
        .restart_agent(old.id, Actor::user())
        .expect("restart");

    let err = h
        .runtime
        .wake_agent(old.id, Actor::user())
        .expect_err("a restarted agent has a successor");
    assert!(err.to_string().contains(&fresh.id.to_string()), "{err}");
    let err = h
        .runtime
        .wake_agent(fresh.id, Actor::user())
        .expect_err("a live agent is not woken");
    assert!(err.to_string().contains("only a failed agent"), "{err}");
    // Nor is the row patched: any update belongs to the successor.
    let err = h
        .runtime
        .update_agent(
            old.id,
            AgentUpdate {
                max_turns: Some(10),
                name: Some("must not rename historical row".into()),
                ..Default::default()
            },
            Actor::user(),
        )
        .expect_err("a restarted agent is not updated");
    assert!(err.to_string().contains(&fresh.id.to_string()), "{err}");
    let still = h.store.agent(old.id).expect("agent").expect("exists");
    assert_eq!(still.status.kind(), AgentStatusKind::Failed);
    assert_eq!(still.config.max_turns, old.config.max_turns);
}

/// A failed agent can be archived like any finished one; the wake lifts the
/// archive, since a live agent is never hidden from the tree.
#[tokio::test]
async fn a_wake_lifts_the_archive_it_finds() {
    let h = harness(vec![
        ScriptedClient::tool_call("fail_task", json!({"error": "the index is corrupt"})),
        ScriptedClient::text("Back."),
    ])
    .await;
    let (agent, _) = h
        .runtime
        .create_agent(spec("index the repository"), Actor::user())
        .expect("create");
    h.wait_until(agent.id, |s| s.kind() == AgentStatusKind::Failed)
        .await;
    let archived = h
        .runtime
        .archive_agents(ArchiveTarget::Agent(agent.id), true, Actor::user())
        .expect("a failed agent can be archived");
    assert!(archived.iter().all(|a| a.archived));

    let woken = h.runtime.wake_agent(agent.id, Actor::user()).expect("wake");
    assert!(!woken.archived);
    assert!(woken.is_live());
    let status = h
        .wait_until(agent.id, |s| s.kind() == AgentStatusKind::Waiting)
        .await;
    assert!(!status.is_terminal(), "{status:?}");
    let stored = h.store.agent(agent.id).expect("agent").expect("exists");
    assert!(!stored.archived, "a live agent is never archived");
    let archive_events = types(h.store.as_ref(), agent.id)
        .into_iter()
        .filter(|t| t == "agent.archived")
        .count();
    assert_eq!(archive_events, 2, "archived once, unarchived by the wake");
}
