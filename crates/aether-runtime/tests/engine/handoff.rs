//! Tool transcripts, model changes and session handoffs.
use super::*;
/// §6.3: what the engine saw has to be in the transcript, or a handoff to an
/// API model would present the engine's tool calls as never having run.
#[tokio::test]
async fn a_tool_the_engine_ran_leaves_its_result_in_the_transcript() {
    let h = harness(Reply::ToolRun).await;
    let id = chat_agent(&h);
    wait_for(h.store.as_ref(), id, |a| {
        a.status.kind() == AgentStatusKind::Waiting
    })
    .await;
    h.runtime
        .send_message(id, vec![ContentBlock::text("what is here?")], Actor::user())
        .expect("send");
    wait_for(h.store.as_ref(), id, |a| {
        a.status.kind() == AgentStatusKind::Waiting && a.stats.turns >= 1
    })
    .await;

    let entries = h.store.transcript(id, 0).expect("transcript");
    let called = entries.iter().any(|e| {
        e.message.content.iter().any(
            |b| matches!(b, aether_core::model::ContentBlock::ToolUse { id, .. } if id == "call-1"),
        )
    });
    let answered = entries.iter().any(|e| {
        e.message.content.iter().any(|b| {
            matches!(b, aether_core::model::ContentBlock::ToolResult { tool_use_id, content, .. }
                if tool_use_id == "call-1"
                    && content.iter().any(|c| matches!(c, aether_core::model::ToolResultBlock::Text { text } if text.contains("src/"))))
        })
    });
    let stored = format!("{entries:?}");
    assert!(
        !stored.contains("NOTAREALKEY") && stored.contains("[REDACTED"),
        "the tool output's key is masked in the transcript"
    );
    assert!(called, "the engine's tool call is in the transcript");
    assert!(
        answered,
        "and so is its result: {:?}",
        entries.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

/// §6.3, the other direction: an agent handed to an engine starts from the
/// checkpoint the previous model left, because the engine has no session yet.
#[tokio::test]
async fn an_engine_taking_over_is_told_the_checkpoint() {
    use aether_core::checkpoint::{Checkpoint, CheckpointDraft, CheckpointProvenance};
    use aether_core::id::CheckpointId;
    use aether_store::{Commit, Mutation};

    let h = harness(Reply::Accept).await;
    let id = chat_agent(&h);
    let agent = wait_for(h.store.as_ref(), id, |a| {
        a.status.kind() == AgentStatusKind::Waiting
    })
    .await;
    assert!(
        agent.engine_session.is_none(),
        "no engine session yet, so the checkpoint is news to it"
    );
    h.store
        .commit(Commit::new().mutate(Mutation::PutCheckpoint(Checkpoint {
            id: CheckpointId::new(),
            agent_id: id,
            version: 1,
            draft: CheckpointDraft {
                objective: "finish the migration".into(),
                state: "three of five tables are migrated".into(),
                constraints: vec!["never drop a table".into()],
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
            created_at: aether_core::time::now(),
        })))
        .expect("seed checkpoint");

    h.runtime
        .send_message(id, vec![ContentBlock::text("carry on")], Actor::user())
        .expect("send");
    wait_for(h.store.as_ref(), id, |a| {
        a.status.kind() == AgentStatusKind::Waiting && a.stats.turns >= 1
    })
    .await;

    // The checkpoint is in the user turn now; what matters is that the
    // engine is still told it, not which half of the input carries it.
    let prompts = h.prompts.lock().unwrap().clone();
    let first = prompts.first().expect("one turn ran");
    assert!(
        first.contains("three of five tables are migrated"),
        "the engine is told what the previous model knew: {first}"
    );
    assert!(
        first.contains("never drop a table"),
        "including the constraints it must keep: {first}"
    );
}

/// The one loss a handoff exists to report: the engine kept a history Aether
/// never saw. The move itself clears `engine_session`, so by the time the fold
/// runs the agent row cannot say there was one - the trigger carries it.
#[tokio::test]
async fn a_handoff_off_an_engine_reports_the_session_it_left_behind() {
    use aether_runtime::AgentUpdate;

    let h = harness(Reply::Accept).await;
    let id = chat_agent(&h);
    h.runtime
        .send_message(id, vec![ContentBlock::text("hello")], Actor::user())
        .expect("send");
    let agent = wait_for(h.store.as_ref(), id, |a| {
        a.status.kind() == AgentStatusKind::Waiting && a.stats.turns >= 1
    })
    .await;
    assert_eq!(
        agent.engine_session.as_deref(),
        Some("s1"),
        "the engine turn left a session behind"
    );

    h.runtime
        .update_agent(
            id,
            AgentUpdate {
                model: Some(ModelRef::new("codex", "gpt-6.1-sol")),
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("hand over to the other engine");
    h.runtime
        .send_message(id, vec![ContentBlock::text("continue")], Actor::user())
        .expect("send after switching");
    wait_for(h.store.as_ref(), id, |a| a.stats.turns >= 2).await;

    let lost = h
        .store
        .events(0, 10_000, &EventFilter::for_agent(id))
        .expect("events")
        .into_iter()
        .find_map(|e| match e.kind {
            EventKind::ContextHandoff { lost, .. } => Some(lost),
            _ => None,
        })
        .expect("the handoff is an event");
    assert!(
        lost.iter().any(|l| l.contains("session s1")),
        "the handoff claims the engine's own history crossed over: {lost:?}"
    );
}

/// A model change during a running turn clears the engine session, and the
/// turn in flight must not write it back. The turn holds a snapshot from
/// before the change, so the session, the model and the config are the
/// store's to keep - reinstating the session resumes the next turn on one the
/// new engine does not own and withholds the checkpoint that replaced it.
#[tokio::test]
async fn a_model_change_during_a_turn_survives_the_turn_finishing() {
    use aether_runtime::AgentUpdate;

    let h = harness(Reply::Accept).await;
    let id = chat_agent(&h);
    let api = ModelRef::new("anthropic", "claude-sonnet-5-5");
    wait_for(h.store.as_ref(), id, |a| {
        a.status.kind() == AgentStatusKind::Waiting
    })
    .await;
    h.runtime
        .send_message(id, vec![ContentBlock::text("hello")], Actor::user())
        .expect("send");
    // The engine opens its session first thing and keeps the turn open after.
    let running = wait_for(h.store.as_ref(), id, |a| a.engine_session.is_some()).await;
    assert_eq!(running.status.kind(), AgentStatusKind::Running);

    // Held, so no later turn runs against a provider this harness lacks and
    // opens a session of its own.
    h.runtime
        .pause_agent(id, false, Actor::user())
        .expect("pause");
    h.runtime
        .update_agent(
            id,
            AgentUpdate {
                model: Some(api.clone()),
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("switch mid-turn");
    assert!(
        h.store
            .agent(id)
            .expect("a")
            .expect("exists")
            .engine_session
            .is_none(),
        "the switch did not clear the session"
    );

    let after = wait_for(h.store.as_ref(), id, |a| {
        a.stats.turns >= 1 && a.status.kind() != AgentStatusKind::Running
    })
    .await;
    assert_eq!(
        after.engine_session, None,
        "the turn put back the session the model change cleared"
    );
    assert_eq!(
        after.model, api,
        "the turn wrote the operator's model change away"
    );
}

/// The return trip. An agent that ran on this engine, moved to an API model
/// and came back cannot be resumed on its old session: everything the API
/// stint did happened outside it. The session is dropped when the model moves
/// away, so the way back is a fresh session that is told the checkpoint.
#[tokio::test]
async fn an_engine_taken_back_gets_a_fresh_session_and_the_checkpoint() {
    use aether_core::checkpoint::{Checkpoint, CheckpointDraft, CheckpointProvenance};
    use aether_core::id::CheckpointId;
    use aether_runtime::AgentUpdate;
    use aether_store::{Commit, Mutation};

    let h = harness(Reply::Accept).await;
    let id = chat_agent(&h);
    let engine = ModelRef::new("claude-code", "haiku");
    let api = ModelRef::new("anthropic", "claude-sonnet-5-5");
    h.runtime
        .send_message(id, vec![ContentBlock::text("start")], Actor::user())
        .expect("send");
    let agent = wait_for(h.store.as_ref(), id, |a| {
        a.status.kind() == AgentStatusKind::Waiting && a.stats.turns >= 1
    })
    .await;
    assert_eq!(
        agent.engine_session.as_deref(),
        Some("s1"),
        "the engine turn left a session to resume"
    );

    // Held, so the move to the API model schedules no turn against a
    // provider this harness does not have.
    h.runtime
        .pause_agent(id, false, Actor::user())
        .expect("pause");
    h.runtime
        .update_agent(
            id,
            AgentUpdate {
                model: Some(api),
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("switch away");
    assert!(
        h.store
            .agent(id)
            .expect("a")
            .expect("exists")
            .engine_session
            .is_none(),
        "the session the engine owns outlived the model that owned it"
    );
    h.store
        .commit(Commit::new().mutate(Mutation::PutCheckpoint(Checkpoint {
            id: CheckpointId::new(),
            agent_id: id,
            version: 1,
            draft: CheckpointDraft {
                objective: "finish the migration".into(),
                state: "the API model migrated the fourth table".into(),
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
            created_at: aether_core::time::now(),
        })))
        .expect("seed checkpoint");
    h.runtime
        .update_agent(
            id,
            AgentUpdate {
                model: Some(engine),
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("switch back");
    h.runtime
        .resume_agent(id, false, Actor::user())
        .expect("resume");
    h.runtime
        .send_message(id, vec![ContentBlock::text("carry on")], Actor::user())
        .expect("send");
    wait_for(h.store.as_ref(), id, |a| {
        a.status.kind() == AgentStatusKind::Waiting && a.stats.turns >= 2
    })
    .await;

    let prompts = h.prompts.lock().unwrap().clone();
    let last = prompts.last().expect("the return turn ran");
    assert!(
        last.contains("the API model migrated the fourth table"),
        "the engine came back to its old session and never read the checkpoint: {last}"
    );
}
