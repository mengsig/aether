//! message fanout scenarios and fixtures.
use super::*;

/// VISION §3.5: a person can fan a message out to an agent's whole live
/// subtree; each delivery is its own inbox item and event, order is preserved
/// per agent, and nothing is delivered twice.
#[tokio::test]
async fn a_message_fans_out_to_every_live_agent_in_the_subtree() {
    let h = harness(vec![]).await;
    let idle = || {
        let mut s = spec("idle");
        s.objective = None;
        s.config.as_mut().expect("config").long_lived = true;
        s
    };
    let (root, _) = h.runtime.create_agent(idle(), Actor::user()).expect("root");
    let mut child_spec = idle();
    child_spec.parent = Some(root.id);
    let (child, _) = h
        .runtime
        .create_agent(child_spec.clone(), Actor::user())
        .expect("child");
    let (gone, _) = h
        .runtime
        .create_agent(child_spec, Actor::user())
        .expect("second child");
    let mut deep_spec = idle();
    deep_spec.parent = Some(child.id);
    let (deep, _) = h
        .runtime
        .create_agent(deep_spec, Actor::user())
        .expect("grandchild");
    let (bystander, _) = h
        .runtime
        .create_agent(idle(), Actor::user())
        .expect("bystander");
    h.runtime
        .cancel_agent(gone.id, Some("done with it".into()), Actor::user())
        .expect("cancel");
    h.wait_until(gone.id, |s| s.kind() == AgentStatusKind::Cancelled)
        .await;

    let first = h
        .runtime
        .fan_out_message(
            root.id,
            vec![ContentBlock::text("stand down for a moment")],
            Actor::user(),
        )
        .expect("fan out");
    assert_eq!(
        first.iter().map(|d| d.agent).collect::<Vec<_>>(),
        vec![root.id, child.id, deep.id],
        "every live agent in the subtree, once each"
    );
    assert_eq!(
        first
            .iter()
            .map(|d| d.message)
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        3,
        "each delivery is its own message"
    );

    // Order per agent, across two fan-outs.
    let second = h
        .runtime
        .fan_out_message(root.id, vec![ContentBlock::text("carry on")], Actor::user())
        .expect("fan out again");
    assert_eq!(second.len(), 3);
    for id in [root.id, child.id, deep.id] {
        let inbox = h.store.inbox(id, 0).expect("inbox");
        let texts: Vec<String> = inbox
            .iter()
            .filter_map(|i| match &i.kind {
                aether_core::inbox::InboxKind::Message { content, .. } => {
                    Some(aether_core::model::text_of(content))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            texts,
            vec!["stand down for a moment", "carry on"],
            "order preserved per agent, nothing delivered twice"
        );
        let mut seqs: Vec<u64> = inbox.iter().map(|i| i.seq).collect();
        let sorted = {
            let mut s = seqs.clone();
            s.sort_unstable();
            s
        };
        assert_eq!(seqs, sorted, "inbox seqs ascend");
        seqs.dedup();
        assert_eq!(seqs.len(), inbox.len(), "no duplicate inbox seq");
        // EventFilter::agent also matches children (parent_agent_id), so count
        // only what is recorded on this agent itself.
        let own: Vec<String> = h
            .store
            .events(0, 10_000, &EventFilter::for_agent(id))
            .expect("events")
            .into_iter()
            .filter(|e| e.agent_id == Some(id))
            .map(|e| e.type_name())
            .collect();
        assert_eq!(
            own.iter().filter(|t| *t == "message.sent").count(),
            2,
            "one event per delivery"
        );
        assert_eq!(own.iter().filter(|t| *t == "inbox.delivered").count(), 2);
    }
    for id in [gone.id, bystander.id] {
        assert!(
            h.store.inbox(id, 0).expect("inbox").is_empty(),
            "a fan-out reaches only the live subtree"
        );
    }

    // A subtree is rooted where you point it.
    let below = h
        .runtime
        .fan_out_message(
            child.id,
            vec![ContentBlock::text("just you two")],
            Actor::user(),
        )
        .expect("fan out below");
    assert_eq!(
        below.iter().map(|d| d.agent).collect::<Vec<_>>(),
        vec![child.id, deep.id]
    );
    let err = h
        .runtime
        .fan_out_message(gone.id, vec![ContentBlock::text("hello?")], Actor::user())
        .expect_err("a cancelled root");
    assert!(err.to_string().contains("cancelled"), "{err}");
}

/// The `send_message` tool with `subtree: true` reaches the recipient and
/// everything live beneath it, not just the recipient.
#[tokio::test]
async fn the_send_message_tool_fans_out_to_a_subtree_when_asked() {
    let h = harness(vec![]).await;
    let idle = || {
        let mut s = spec("idle");
        s.objective = None;
        s.config.as_mut().expect("config").long_lived = true;
        s
    };
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
    let (child, _) = h
        .runtime
        .create_agent(idle(), Actor::user())
        .expect("child");
    let mut grandchild_spec = idle();
    grandchild_spec.parent = Some(child.id);
    let (grandchild, _) = h
        .runtime
        .create_agent(grandchild_spec, Actor::user())
        .expect("grandchild");

    // The script needs the child id, so it is queued only once the tree exists.
    h.client.push(ScriptedClient::tool_call(
        "send_message",
        json!({"to": child.id.to_string(), "content": "stand down", "subtree": true}),
    ));
    h.client.push(ScriptedClient::tool_call(
        "complete_task",
        json!({"summary": "broadcast sent"}),
    ));
    h.runtime
        .create_task(
            aether_runtime::CreateTaskSpec {
                objective: "tell the branch to stand down".into(),
                owner: Some(sender.id),
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("task");
    h.wait_until(sender.id, |s| s.is_terminal()).await;

    for id in [child.id, grandchild.id] {
        let texts: Vec<String> = h
            .store
            .inbox(id, 0)
            .expect("inbox")
            .iter()
            .filter_map(|i| match &i.kind {
                aether_core::inbox::InboxKind::Message { content, .. } => {
                    Some(aether_core::model::text_of(content))
                }
                _ => None,
            })
            .collect();
        assert_eq!(texts, vec!["stand down"], "subtree delivery reached {id}");
    }
}

/// `fail_task` carries `structured` all the way to the parent's inbox, and the
/// rendered inbox item shows it - the mirror of `complete_task`.
#[tokio::test]
async fn a_structured_failure_reaches_the_parent_inbox_and_its_rendering() {
    let h = harness(vec![
        // parent turn 1: delegate and wait for the child
        ScriptedClient::tool_call(
            "delegate",
            json!({"model":"anthropic/claude-sonnet-5-5", "effort":"low", "role": "worker", "objective": "do the sub task", "wait": true}),
        ),
        // child turn 1: fail with machine-readable detail
        ScriptedClient::tool_call(
            "fail_task",
            json!({"error": "the migration never converged", "structured": {"attempts": 3}}),
        ),
        // parent turn 2: woken by the failure
        ScriptedClient::tool_call("complete_task", json!({"summary": "reported the failure"})),
    ])
    .await;
    let (parent, _) = h
        .runtime
        .create_agent(spec("split the work"), Actor::user())
        .expect("create");
    h.wait_until(parent.id, |s| s.is_terminal()).await;

    let items = h.store.inbox(parent.id, 0).expect("inbox");
    let failure = items
        .iter()
        .find(|i| matches!(i.kind, aether_core::inbox::InboxKind::TaskFailed { .. }))
        .expect("the parent was told its child's task failed");
    let aether_core::inbox::InboxKind::TaskFailed {
        error, structured, ..
    } = &failure.kind
    else {
        unreachable!()
    };
    assert_eq!(error, "the migration never converged");
    assert_eq!(structured.as_ref(), Some(&json!({"attempts": 3})));

    let (message, _, _) =
        aether_context::render::inbox_message(std::slice::from_ref(failure), None);
    let rendered = aether_core::model::text_of(&message.expect("rendered").content);
    assert!(
        rendered.contains("the migration never converged")
            && rendered.contains(r#"data: {"attempts":3}"#),
        "the rendering carries the structured data: {rendered}"
    );
}
