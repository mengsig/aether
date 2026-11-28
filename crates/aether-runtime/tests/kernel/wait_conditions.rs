//! wait conditions scenarios and fixtures.
use super::*;

/// An operator-configured wait timeout adds a deadline, and at it the
/// agent wakes with a note of what is still open.
#[tokio::test]
async fn a_wait_deadline_wakes_the_agent_with_what_is_still_open() {
    let h = harness_configured(
        vec![
            // parent: delegate and wait on a child that never reports back
            ScriptedClient::tool_call(
                "delegate",
                json!({"model":"anthropic/claude-sonnet-5-5", "effort":"low", "role": "worker", "objective": "never finishes", "wait": true}),
            ),
            // child: park on a question, so its task stays open
            ScriptedClient::tool_call("ask_user", json!({"question": "which one?"})),
            // parent, woken by the deadline
            ScriptedClient::text("The worker has not answered; I will chase it."),
        ],
        |_| {},
        |c| {
            c.max_concurrent_turns = 4;
            c.wait_timeout = Some(Duration::from_secs(1));
        },
    )
    .await;
    let (parent, _) = h
        .runtime
        .create_agent(spec("split the work"), Actor::user())
        .expect("parent");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    let note = loop {
        let woken = h
            .store
            .inbox(parent.id, 0)
            .expect("inbox")
            .into_iter()
            .find_map(|i| match i.kind {
                aether_core::inbox::InboxKind::Wake { reason } => Some(reason),
                _ => None,
            });
        if let Some(note) = woken {
            break note;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the parent never woke at its deadline"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    let child_task = h
        .store
        .tasks(&aether_store::TaskFilter::default())
        .expect("tasks")
        .into_iter()
        .find(|t| t.objective == "never finishes")
        .expect("child task");
    assert!(
        note.contains(&child_task.id.to_string()),
        "the note does not name the open task: {note}"
    );
    assert!(note.contains("timed out"), "{note}");
    // Woken, it takes a turn rather than sleeping on forever. Spare replies
    // cover the wait it re-enters, which times out again on the same clock.
    for _ in 0..5 {
        h.client.push(ScriptedClient::text("still chasing"));
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let parent = h.store.agent(parent.id).expect("a").expect("e");
        if parent.stats.turns >= 2 {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the parent never took a turn after waking: {:?}",
            parent.status
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// A wait on a named sender wakes for that agent's message and no other. The
/// agent builds the wait itself, through `wait_for`'s `message_from`, so the
/// status it parks in is one the validated state machine actually produced.
#[tokio::test]
async fn a_wait_on_a_named_sender_wakes_only_for_that_agent() {
    use aether_core::agent::WaitCondition;
    let h = harness(vec![]).await;
    let idle = |name: &str| CreateAgentSpec {
        objective: None,
        name: Some(name.to_owned()),
        ..spec("unused")
    };
    let (listener, _) = h
        .runtime
        .create_agent(idle("listener"), Actor::user())
        .expect("listener");
    let (other, _) = h
        .runtime
        .create_agent(idle("other"), Actor::user())
        .expect("other");
    let (expected_agent, _) = h
        .runtime
        .create_agent(idle("expected"), Actor::user())
        .expect("expected");

    // The listener parks itself on a message from one named agent.
    h.client.push(ScriptedClient::tool_calls(
        &[(
            "wait_for",
            json!({ "message_from": expected_agent.id.to_string() }),
        )],
        Some("I will wait for the expert."),
    ));
    h.runtime
        .send_message(
            listener.id,
            vec![ContentBlock::text("wait for the expert")],
            Actor::user(),
        )
        .expect("send");
    let parked = h
        .wait_until(listener.id, |s| {
            matches!(s, AgentStatus::Waiting { condition }
            if condition.message_senders().contains(&expected_agent.id))
        })
        .await;
    let AgentStatus::Waiting { condition } = &parked else {
        panic!("{parked:?}");
    };
    assert_eq!(
        condition,
        &WaitCondition::MessageFrom {
            agent: expected_agent.id
        },
        "an event wait must not acquire a periodic timer"
    );
    let requests_when_parked = h.client.requests().len();
    tokio::time::sleep(Duration::from_millis(1100)).await;
    assert_eq!(h.client.requests().len(), requests_when_parked);
    let turns_when_parked = h
        .store
        .agent(listener.id)
        .expect("a")
        .expect("e")
        .stats
        .turns;

    h.runtime
        .send_message(
            listener.id,
            vec![ContentBlock::text("not the one")],
            Actor::agent(other.id),
        )
        .expect("send");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        h.store.agent(listener.id).expect("a").expect("e").status,
        parked,
        "another agent's message woke a wait on a named sender"
    );

    h.client.push(ScriptedClient::text("thanks"));
    h.runtime
        .send_message(
            listener.id,
            vec![ContentBlock::text("the one")],
            Actor::agent(expected_agent.id),
        )
        .expect("send");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        let a = h.store.agent(listener.id).expect("a").expect("e");
        if a.stats.turns > turns_when_parked {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the named sender's message never woke the agent: {:?}",
            a.status
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    // Exactly one wake, carrying both messages: the wrong sender's message
    // was delivered but never on its own worth a turn.
    let woken = h
        .store
        .agent(listener.id)
        .expect("a")
        .expect("e")
        .stats
        .turns;
    assert_eq!(woken, turns_when_parked + 1, "the wrong sender woke it too");
    let last = h.client.requests().pop().expect("a request");
    let text: String = last.messages.iter().map(|m| m.text()).collect();
    assert!(text.contains("not the one"), "{text}");
    assert!(text.contains("the one"), "{text}");
}

/// The deadline note names each branch of the wait in its own words: a task
/// does not arrive and a message does not finish.
#[tokio::test]
async fn a_deadline_note_names_the_open_task_and_the_silent_sender() {
    let h = harness(vec![]).await;
    let (listener, _) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                objective: None,
                ..spec("unused")
            },
            Actor::user(),
        )
        .expect("listener");
    let (sender, _) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                objective: None,
                ..spec("unused")
            },
            Actor::user(),
        )
        .expect("sender");
    // An ownerless task is never scheduled, so it stays open for the wait.
    let open = h
        .runtime
        .create_task(
            aether_runtime::CreateTaskSpec {
                objective: "nobody is doing this".into(),
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("open task");

    h.client.push(ScriptedClient::tool_calls(
        &[(
            "wait_for",
            json!({
                "tasks": [open.id.to_string()],
                "message_from": sender.id.to_string(),
                "timeout_seconds": 1,
            }),
        )],
        Some("I will wait for either result."),
    ));
    h.client.push(ScriptedClient::text("I will chase both."));
    h.runtime
        .send_message(
            listener.id,
            vec![ContentBlock::text("wait for either")],
            Actor::user(),
        )
        .expect("send");

    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    let note = loop {
        let woken = h
            .store
            .inbox(listener.id, 0)
            .expect("inbox")
            .into_iter()
            .find_map(|i| match i.kind {
                aether_core::inbox::InboxKind::Wake { reason } => Some(reason),
                _ => None,
            });
        if let Some(note) = woken {
            break note;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the wait never timed out"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert!(note.contains("timed out"), "{note}");
    assert!(
        note.contains(&format!("task {} still has not finished", open.id)),
        "a task finishes, it does not arrive: {note}"
    );
    assert!(
        note.contains(&format!(
            "a message from {} still has not arrived",
            sender.id
        )),
        "the silent sender is not named: {note}"
    );
}

/// `wait_for` must reject a task id bound to an agent outside the
/// caller's own subtree - it must not silently park on someone else's task
/// forever. An ownerless task created directly by a user/system actor (no
/// agent bound to it at all) stays valid input, covered above by
/// `a_deadline_note_names_the_open_task_and_the_silent_sender`.
#[tokio::test]
async fn wait_for_rejects_a_task_outside_the_callers_scope() {
    let h = harness(vec![]).await;
    let idle = |name: &str| CreateAgentSpec {
        objective: None,
        name: Some(name.to_owned()),
        ..spec("unused")
    };
    let (other, _) = h
        .runtime
        .create_agent(idle("other"), Actor::user())
        .expect("other");
    // Bound to `other` as creator, not owned by anyone - so it is never
    // scheduled and `other` never takes a turn, but it is still `other`'s,
    // not the waiter's.
    let foreign = h
        .runtime
        .create_task(
            aether_runtime::CreateTaskSpec {
                objective: "someone else's work".into(),
                ..Default::default()
            },
            Actor::Agent { id: other.id },
        )
        .expect("foreign task");
    let (waiter, _) = h
        .runtime
        .create_agent(idle("waiter"), Actor::user())
        .expect("waiter");

    h.client.push(ScriptedClient::tool_calls(
        &[("wait_for", json!({"tasks": [foreign.id.to_string()]}))],
        Some("I will check that task."),
    ));
    h.client.push(ScriptedClient::text("understood"));
    h.runtime
        .send_message(waiter.id, vec![ContentBlock::text("go")], Actor::user())
        .expect("send");

    h.wait_for("the waiter's turn to finish", || h.client.remaining() == 0)
        .await;

    let entries = h.store.transcript(waiter.id, 0).expect("transcript");
    let rejection = entries.iter().find_map(|e| {
        e.message.content.iter().find_map(|b| match b {
            ContentBlock::ToolResult {
                content,
                is_error: true,
                ..
            } => content.iter().find_map(|c| match c {
                aether_core::model::ToolResultBlock::Text { text }
                    if text.contains("wait_for.tasks") =>
                {
                    Some(text.clone())
                }
                _ => None,
            }),
            _ => None,
        })
    });
    let text = rejection.expect("wait_for on a foreign task was accepted, not rejected");
    assert!(text.contains(&foreign.id.to_string()), "{text}");
    assert!(text.contains("may not wait on"), "{text}");
}

/// The silent-sender check reads the caller's inbox; when that read fails the
/// wait is an error the model sees, never a guess that nothing was delivered.
#[tokio::test]
async fn a_failed_inbox_read_during_wait_validation_fails_the_turn_visibly() {
    let inner: Arc<dyn Store> = Arc::new(SqliteStore::open_in_memory().expect("store"));
    let store = Arc::new(CommitsMidPass::new(inner, Hook::InboxRead));
    let h = harness_full(
        Vec::new(),
        |_| {},
        |_| {},
        None,
        None,
        Some(store.clone()),
        |_| {},
    )
    .await;
    let (node, _) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                role: "pipeline:run".into(),
                start: false,
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("run node");
    h.client.push(ScriptedClient::tool_call(
        "wait_for",
        json!({"message_from": node.id.to_string()}),
    ));
    let gate = h.client.hold_response_at(1);
    let (agent, _) = h
        .runtime
        .create_agent(spec("wait on the node"), Actor::user())
        .expect("create");
    h.wait_for("the turn reached the model", || {
        h.client.requests().len() == 1
    })
    .await;
    store.fail_inbox_reads();
    gate.cancel();

    let events = wait_until_events(h.store.as_ref(), agent.id, |events| {
        events.iter().any(|e| e.type_name() == "agent.failed")
    })
    .await;
    let error = events
        .iter()
        .find_map(|e| match &e.kind {
            EventKind::AgentFailed { error } => Some(error.clone()),
            _ => None,
        })
        .expect("agent.failed");
    assert!(error.contains("simulated inbox read failure"), "{error}");
    assert!(store.read_failed());
    assert!(
        !events.iter().any(|e| e.type_name() == "tool.succeeded"),
        "a failed read must not settle as a wait or a refusal"
    );
}
