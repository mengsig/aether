use super::*;
use crate::turn::compaction::*;

fn m(name: &str) -> ModelRef {
    ModelRef::new("p", name)
}

fn every_trigger() -> Vec<CompactionTrigger> {
    vec![
        CompactionTrigger::TokenThreshold {
            tokens: 100,
            threshold: 90,
        },
        CompactionTrigger::TaskBoundary {
            task: TaskId::new(),
        },
        CompactionTrigger::LargeToolOutput {
            call: ToolCallId::new(),
        },
        CompactionTrigger::LongWait,
        CompactionTrigger::Explicit { by: Actor::user() },
        CompactionTrigger::Handoff {
            from: m("engine"),
            to: m("strong"),
            session: None,
        },
    ]
}

/// Model selection only. Whether each trigger is ever raised is the
/// readiness matrix's business, and `large_tool_output` and `long_wait`
/// are defined and not yet raised.
#[test]
fn the_cheap_compactor_is_picked_for_every_trigger_with_a_stronger_retry() {
    for trigger in every_trigger() {
        let (primary, fallback) =
            compaction_models(Some(m("cheap")), &m("strong"), &trigger, |_| true)
                .expect("a model can compact");
        assert_eq!(primary, m("cheap"), "{}", trigger.name());
        assert_eq!(
            fallback,
            Some(m("strong")),
            "{} must keep a stronger retry",
            trigger.name()
        );
    }
}

#[test]
fn without_a_configured_compactor_the_agents_own_model_folds_alone() {
    // A handoff is the exception: the outgoing model folds its own context.
    for trigger in every_trigger()
        .into_iter()
        .filter(|t| !matches!(t, CompactionTrigger::Handoff { .. }))
    {
        let (primary, fallback) =
            compaction_models(None, &m("strong"), &trigger, |_| true).expect("a model can compact");
        assert_eq!(primary, m("strong"), "{}", trigger.name());
        // Retrying the same model with the same prompt is not a fallback.
        assert_eq!(fallback, None, "{}", trigger.name());
    }
}

#[test]
fn a_handoff_off_an_engine_folds_with_the_model_taking_over() {
    let trigger = CompactionTrigger::Handoff {
        from: m("engine"),
        to: m("api"),
        session: None,
    };
    let (primary, fallback) =
        compaction_models(None, &m("api"), &trigger, |model| model != &m("engine"))
            .expect("a model can compact");
    assert_eq!(primary, m("api"));
    assert_eq!(fallback, None);
}

#[test]
fn a_handoff_onto_an_engine_folds_with_the_model_handing_over() {
    let trigger = CompactionTrigger::Handoff {
        from: m("api"),
        to: m("engine"),
        session: None,
    };
    let (primary, fallback) =
        compaction_models(None, &m("engine"), &trigger, |model| model != &m("engine"))
            .expect("the outgoing model can still read its own context");
    assert_eq!(primary, m("api"));
    assert_eq!(
        fallback, None,
        "the engine cannot be asked for a checkpoint"
    );
}

#[test]
fn an_engine_agent_with_no_compactor_configured_cannot_compact() {
    let trigger = CompactionTrigger::TaskBoundary {
        task: TaskId::new(),
    };
    assert!(
        compaction_models(None, &m("engine"), &trigger, |model| model != &m("engine")).is_none()
    );
}

#[tokio::test]
async fn explicit_requests_do_not_replace_a_pending_model_handoff() {
    let (runtime, _dir) = crate::test_runtime(|_| {}).await;
    let (agent, _) = runtime
        .create_agent(
            CreateAgentSpec {
                role: "worker".into(),
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("create agent");
    let handoff = CompactionTrigger::Handoff {
        from: m("old"),
        to: m("new"),
        session: Some("old-session".into()),
    };
    runtime
        .apply(
            Commit::new().event(
                EventDraft::new(
                    Actor::user(),
                    EventKind::CompactionRequested {
                        trigger: handoff.clone(),
                    },
                )
                .agent(agent.id)
                .run(agent.run),
            ),
        )
        .expect("request handoff");

    runtime
        .request_compaction(agent.id, Actor::user())
        .expect("explicit operator request");
    let mut ctx = TurnCtx {
        agent,
        task: None,
        turn: 0,
        turn_usage: Default::default(),
        base_actor: Actor::user(),
        hard_cancel: CancellationToken::new(),
        recovery_checked_for: None,
        user_reply_pending: false,
        reply_given_unrecorded: false,
        start_handoff: None,
    };
    let outcome = apply_action(
        &runtime,
        &mut ctx,
        HarnessAction::RequestCheckpoint {
            reason: "manual".into(),
        },
        &mut ToolEffects::default(),
    )
    .await
    .expect("agent request");
    assert!(outcome.content.contains("already requested"));
    assert!(matches!(
        compaction_pending(&runtime, ctx.agent.id).expect("pending request"),
        Some(CompactionTrigger::Handoff { from, to, session })
            if from == m("old") && to == m("new") && session.as_deref() == Some("old-session")
    ));
}

#[test]
fn a_tool_call_the_engine_never_answered_counts_as_lost() {
    let call = |id: &str| ContentBlock::ToolUse {
        id: id.into(),
        name: "bash".into(),
        input: serde_json::json!({}),
    };
    let result = |id: &str| ContentBlock::ToolResult {
        tool_use_id: id.into(),
        content: vec![ToolResultBlock::Text { text: "ok".into() }],
        is_error: false,
    };
    let entry = |seq: u64, role: MessageRole, content: Vec<ContentBlock>| TranscriptEntry {
        seq,
        at: now(),
        message: Message { role, content },
        invocation: None,
    };
    let entries = vec![
        entry(1, MessageRole::Assistant, vec![call("a"), call("b")]),
        entry(2, MessageRole::User, vec![result("a")]),
    ];
    assert_eq!(unanswered_tool_calls(&entries), 1);
    let answered = vec![
        entry(1, MessageRole::Assistant, vec![call("a")]),
        entry(2, MessageRole::User, vec![result("a")]),
    ];
    assert_eq!(unanswered_tool_calls(&answered), 0);
}

#[tokio::test]
async fn compaction_without_usage_reports_a_live_gap() {
    let (mut h, _dir) = ComposeHarness::new().await;
    let attempt = aether_context::CompactionAttempt {
        model: ModelRef::new("claude-code", "haiku"),
        usage: None,
        error: None,
    };
    record_compaction_usage(&h.runtime, &mut h.ctx, &attempt).unwrap();
    assert!(!h.ctx.turn_usage.cost_complete);
    assert_eq!(
        h.ctx.turn_usage.cost_gap,
        Some(aether_core::model::CostGap::Unreported)
    );
    h.runtime.shutdown().await;
}

/// A CLI compactor that failed preflight is refused with that failure's
/// reason, not a generic "not installed".
#[tokio::test]
async fn an_unusable_engine_compactor_is_refused_with_its_probe_reason() {
    let (h, _dir) = ComposeHarness::new().await;
    assert_eq!(
        h.ctx.agent.scope.backend,
        aether_core::agent::ExecutionBackend::Host,
        "only a host agent can compact on an engine"
    );
    let kind = aether_engine::EngineKind::CLAUDE_CODE;
    h.runtime
        .engines
        .mark_unavailable(kind, "claude --version exited 1: claude is broken");
    let engine_model = ModelRef::new("claude-code", "haiku");
    let refused = match compaction_client(&h.runtime, &h.ctx, &engine_model).await {
        Ok(_) => panic!("an unusable engine cannot compact"),
        Err(error) => error.to_string(),
    };
    assert!(refused.contains("claude is broken"), "{refused}");
    assert!(!refused.contains("not installed"), "{refused}");
    h.runtime.shutdown().await;
}

/// Delegating onto an engine that failed preflight is refused with that
/// failure's reason and the retry advice, not a generic "not available".
#[tokio::test]
async fn delegating_onto_an_unusable_engine_names_its_probe_reason() {
    let (mut h, _dir) = ComposeHarness::new().await;
    h.runtime.engines.mark_unavailable(
        aether_engine::EngineKind::CLAUDE_CODE,
        "claude --version exited 1: claude is broken",
    );
    let delegate = HarnessAction::Delegate {
        role: "worker".into(),
        name: None,
        objective: "do it".into(),
        instructions: None,
        model: ModelRef::new("claude-code", "haiku"),
        effort: aether_core::model::Effort::Low,
        scope: None,
        workspace_mode: aether_tools::WorkspaceMode::Shared,
        wait: false,
        leaf: true,
        host_context: None,
        idempotency: None,
    };
    let refused = apply_action(
        &h.runtime,
        &mut h.ctx,
        delegate,
        &mut ToolEffects::default(),
    )
    .await
    .expect("a refusal is a tool outcome");
    assert!(refused.is_error, "{}", refused.content);
    assert!(
        refused.content.contains("claude is broken"),
        "{}",
        refused.content
    );
    assert!(
        refused.content.contains("no daemon restart is needed"),
        "{}",
        refused.content
    );
    h.runtime.shutdown().await;
}
