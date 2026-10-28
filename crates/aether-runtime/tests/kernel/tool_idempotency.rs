//! tool idempotency scenarios and fixtures.
use super::*;

#[tokio::test]
async fn an_identical_command_in_a_later_turn_runs_again() {
    // Replay answers a re-driven invocation, not a live agent asking again.
    // The model appends the same line in two successive turns: both must run,
    // or an agent could never observe the effect of its own edits.
    let command = "echo line >> log.txt; cat log.txt";
    let h = harness(vec![
        ScriptedClient::tool_call("bash", json!({ "command": command })),
        ScriptedClient::tool_call("bash", json!({ "command": command })),
        ScriptedClient::tool_call("complete_task", json!({"summary": "appended twice"})),
    ])
    .await;
    let (agent, _) = h
        .runtime
        .create_agent(spec("append a line twice"), Actor::user())
        .expect("create");
    let status = h.wait_until(agent.id, |s| s.is_terminal()).await;
    assert_eq!(
        status.kind(),
        AgentStatusKind::Completed,
        "{:?}",
        types(h.store.as_ref(), agent.id)
    );
    assert_eq!(
        std::fs::read_to_string(h.workspace_root.join("log.txt")).expect("file"),
        "line\nline\n",
        "a fresh call in a later turn runs; it is not answered from a record"
    );
    let events = h
        .store
        .events(0, 10_000, &EventFilter::for_agent(agent.id))
        .expect("events");
    assert!(
        events
            .iter()
            .all(|e| !matches!(e.kind, EventKind::ToolReplayed { .. })),
        "no call was re-driven, so nothing replays"
    );
    let requested: Vec<_> = events
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::ToolRequested { call, .. } if call.name == "bash" => {
                Some((call.id, call.idempotency_key.clone(), call.invocation_id))
            }
            _ => None,
        })
        .collect();
    assert_eq!(requested.len(), 2);
    assert_eq!(
        requested[0].1, requested[1].1,
        "identical input still derives one key"
    );
    assert_ne!(
        requested[0].2, requested[1].2,
        "each turn is its own invocation, which is what keeps them apart"
    );
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(&e.kind, EventKind::ToolSucceeded { call, .. }
                if requested.iter().any(|(id, _, _)| id == call)))
            .count(),
        2
    );
}

#[tokio::test]
async fn two_identical_calls_in_one_batch_both_run() {
    // Two calls in one model response are two deliberate calls within one
    // invocation, not a re-drive: both run, and the record keeps the first.
    let command = "echo line >> log.txt";
    let h = harness(vec![
        ScriptedClient::tool_calls(
            &[
                ("bash", json!({ "command": command })),
                ("bash", json!({ "command": command })),
            ],
            None,
        ),
        ScriptedClient::tool_call("complete_task", json!({"summary": "appended twice"})),
    ])
    .await;
    let (agent, _) = h
        .runtime
        .create_agent(spec("append twice in one batch"), Actor::user())
        .expect("create");
    let status = h.wait_until(agent.id, |s| s.is_terminal()).await;
    assert_eq!(
        status.kind(),
        AgentStatusKind::Completed,
        "{:?}",
        types(h.store.as_ref(), agent.id)
    );
    assert_eq!(
        std::fs::read_to_string(h.workspace_root.join("log.txt")).expect("file"),
        "line\nline\n",
        "both calls in the batch take effect"
    );
    let events = h
        .store
        .events(0, 10_000, &EventFilter::for_agent(agent.id))
        .expect("events");
    assert!(
        events
            .iter()
            .all(|e| !matches!(e.kind, EventKind::ToolReplayed { .. })),
        "a same-batch duplicate is run, not replayed"
    );
}

#[tokio::test]
async fn a_sibling_calls_committed_outcome_never_answers_this_call() {
    // Two identical gated calls in one response, answered in separate
    // rounds: the first has committed its outcome under the same key and
    // invocation by the time the second settles. The second is still a
    // deliberate call the person approved, so it must run.
    let h = harness(vec![
        ScriptedClient::tool_call(
            "write_file",
            json!({"path": "note.txt", "content": "hello"}),
        ),
        ScriptedClient::tool_call("complete_task", json!({"summary": "written"})),
    ])
    .await;
    let mut s = spec("write a note");
    s.config = Some(AgentConfig {
        approval: ApprovalPolicy {
            require_for: [SideEffectClass::IdempotentMutation].into(),
        },
        ..Default::default()
    });
    let (agent, _) = h.runtime.create_agent(s, Actor::user()).expect("create");
    h.wait_until(agent.id, |s| s.kind() == AgentStatusKind::Waiting)
        .await;
    let pending = h.store.pending_approvals().expect("pending");
    assert_eq!(pending.len(), 1);
    let call = h
        .store
        .events(0, 10_000, &EventFilter::for_agent(agent.id))
        .expect("events")
        .into_iter()
        .find_map(|e| match e.kind {
            EventKind::ToolRequested { call, .. } if call.name == "write_file" => Some(call),
            _ => None,
        })
        .expect("the write was requested");
    // The sibling call: same agent, same invocation, same key, its own id.
    h.store
        .commit(
            Commit::new().mutate(Mutation::RecordToolOutcome(RecordedOutcome {
                key: call.idempotency_key.clone().expect("a mutation is keyed"),
                agent_id: call.agent_id,
                invocation_id: call.invocation_id.expect("a call names its invocation"),
                call_id: ToolCallId::new(),
                tool: call.name.clone(),
                at: aether_core::time::now(),
                outcome: ToolOutcome::ok("the sibling wrote it"),
            })),
        )
        .expect("record");

    h.runtime
        .resolve_approval(
            pending[0].id,
            ApprovalDecision::Approved,
            Actor::user(),
            None,
            None,
        )
        .expect("approve");
    h.wait_until(agent.id, |s| s.is_terminal()).await;
    assert_eq!(
        std::fs::read_to_string(h.workspace_root.join("note.txt")).expect("file"),
        "hello",
        "an approved call answered from another call's record never happens"
    );
    let events = h
        .store
        .events(0, 10_000, &EventFilter::for_agent(agent.id))
        .expect("events");
    assert!(
        events
            .iter()
            .all(|e| !matches!(e.kind, EventKind::ToolReplayed { .. })),
        "another call's outcome is not a replay of this one"
    );
}

#[tokio::test]
async fn a_re_driven_call_replays_its_committed_outcome_instead_of_repeating_it() {
    // The outcome of this call was already committed while driving its
    // invocation. Settling the approval re-drives the same call, so it must
    // be answered from the record rather than write the file a second time.
    let h = harness(vec![
        ScriptedClient::tool_call(
            "write_file",
            json!({"path": "note.txt", "content": "hello"}),
        ),
        ScriptedClient::tool_call("complete_task", json!({"summary": "already done"})),
    ])
    .await;
    let mut s = spec("write a note");
    s.config = Some(AgentConfig {
        approval: ApprovalPolicy {
            require_for: [SideEffectClass::IdempotentMutation].into(),
        },
        ..Default::default()
    });
    let (agent, _) = h.runtime.create_agent(s, Actor::user()).expect("create");
    h.wait_until(agent.id, |s| s.kind() == AgentStatusKind::Waiting)
        .await;
    let pending = h.store.pending_approvals().expect("pending");
    assert_eq!(pending.len(), 1);

    // The call as recorded, with the invocation that produced it.
    let call = h
        .store
        .events(0, 10_000, &EventFilter::for_agent(agent.id))
        .expect("events")
        .into_iter()
        .find_map(|e| match e.kind {
            EventKind::ToolRequested { call, .. } if call.name == "write_file" => Some(call),
            _ => None,
        })
        .expect("the write was requested");
    // A re-drive replays the recorded call, id and all; the record is keyed
    // by that id, so it answers this call and no other. It holds a token the
    // runtime masks now, as a record made before masking would.
    let token = format!("ghp_{}", "A".repeat(36));
    let mut recorded = ToolOutcome::ok(format!("already written with {token}"));
    recorded.structured = Some(json!({ "token": token }));
    h.store
        .commit(
            Commit::new().mutate(Mutation::RecordToolOutcome(RecordedOutcome {
                key: call.idempotency_key.clone().expect("a mutation is keyed"),
                agent_id: call.agent_id,
                invocation_id: call.invocation_id.expect("a call names its invocation"),
                call_id: call.id,
                tool: call.name.clone(),
                at: aether_core::time::now(),
                outcome: recorded,
            })),
        )
        .expect("record");

    h.runtime
        .resolve_approval(
            pending[0].id,
            ApprovalDecision::Approved,
            Actor::user(),
            None,
            None,
        )
        .expect("approve");
    h.wait_until(agent.id, |s| s.is_terminal()).await;
    assert!(
        !h.workspace_root.join("note.txt").exists(),
        "a re-driven call must not repeat its effect"
    );
    let events = h
        .store
        .events(0, 10_000, &EventFilter::for_agent(agent.id))
        .expect("events");
    let replayed: Vec<_> = events
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::ToolReplayed { call, original, .. } => Some((*call, *original)),
            _ => None,
        })
        .collect();
    assert_eq!(replayed.len(), 1, "{:?}", types(h.store.as_ref(), agent.id));
    assert_eq!(replayed[0].0, call.id);
    assert_eq!(replayed[0].1, call.id, "the replay names the call that ran");
    let replayed_outcome = events
        .iter()
        .find_map(|e| match &e.kind {
            EventKind::ToolSucceeded { call: id, outcome } if *id == call.id => Some(outcome),
            _ => None,
        })
        .expect("the replay commits the recorded outcome");
    let committed = serde_json::to_string(replayed_outcome).expect("serializes");
    assert!(
        !committed.contains(&token) && committed.contains("[REDACTED:github_token]"),
        "a replayed outcome is masked like a fresh one: {committed}"
    );
}

#[tokio::test]
async fn every_tool_outcome_follows_its_request_in_the_log() {
    // VISION 3.4: the log is causally ordered, so an outcome must never
    // carry a lower sequence than the request it belongs to.
    let h = harness(vec![
        ScriptedClient::tool_call("bash", json!({"command": "echo hi"})),
        ScriptedClient::tool_call("complete_task", json!({"summary": "done"})),
    ])
    .await;
    let (agent, _) = h
        .runtime
        .create_agent(spec("run a command"), Actor::user())
        .expect("create");
    h.wait_until(agent.id, |s| s.is_terminal()).await;
    let events = h
        .store
        .events(0, 10_000, &EventFilter::for_agent(agent.id))
        .expect("events");
    let mut requested: std::collections::BTreeMap<ToolCallId, u64> = Default::default();
    for event in &events {
        match &event.kind {
            EventKind::ToolRequested { call, .. } => {
                requested.insert(call.id, event.seq);
            }
            EventKind::ToolSucceeded { call, .. }
            | EventKind::ToolReplayed { call, .. }
            | EventKind::ToolStarted { call } => {
                let at = requested.get(call).copied().unwrap_or(u64::MAX);
                assert!(
                    at < event.seq,
                    "{:?} at {} precedes its request",
                    event.kind.type_name(),
                    event.seq
                );
            }
            _ => {}
        }
    }
    assert!(!requested.is_empty(), "the turn made a tool call");
}

#[tokio::test]
async fn a_read_only_call_is_never_keyed_and_runs_every_time() {
    let h = harness(vec![
        ScriptedClient::tool_call("list_dir", json!({"path": "."})),
        ScriptedClient::tool_call("list_dir", json!({"path": "."})),
        ScriptedClient::tool_call("complete_task", json!({"summary": "listed twice"})),
    ])
    .await;
    let (agent, _) = h
        .runtime
        .create_agent(spec("list twice"), Actor::user())
        .expect("create");
    h.wait_until(agent.id, |s| s.is_terminal()).await;
    let events = h
        .store
        .events(0, 10_000, &EventFilter::for_agent(agent.id))
        .expect("events");
    assert!(
        events
            .iter()
            .all(|e| !matches!(e.kind, EventKind::ToolReplayed { .. })),
        "a read is repeated, not replayed"
    );
    assert!(
        events.iter().all(|e| !matches!(&e.kind,
            EventKind::ToolRequested { call, .. } if call.name == "list_dir"
                && call.idempotency_key.is_some())),
        "a read-only call records no key"
    );
}
