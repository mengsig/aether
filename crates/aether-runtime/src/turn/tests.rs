use super::*;
use aether_core::id::TaskId;

mod compaction;
mod heartbeat;
mod lifecycle;
mod resources;
mod stale_completion;
mod subtree_reads;
mod tool_results;
mod woken;

/// A live runtime and agent, plus small builders for the `tool.requested`
/// / `tool.succeeded` / `agent.message` events that `compose_tool_results`
/// and `requested_calls` read straight from the store.
struct ComposeHarness {
    runtime: Arc<Runtime>,
    ctx: TurnCtx,
}

impl ComposeHarness {
    async fn new() -> (Self, tempfile::TempDir) {
        let (runtime, dir) = crate::test_runtime(|_| {}).await;
        let (agent, _task) = runtime
            .create_agent(
                CreateAgentSpec {
                    role: "worker".into(),
                    start: true,
                    ..Default::default()
                },
                Actor::user(),
            )
            .expect("create agent");
        let ctx = TurnCtx {
            agent,
            task: None,
            turn: 0,
            turn_usage: aether_core::model::Usage::default(),
            base_actor: Actor::user(),
            hard_cancel: CancellationToken::new(),
            recovery_checked_for: None,
            user_reply_pending: false,
            reply_given_unrecorded: false,
            start_handoff: None,
        };
        (Self { runtime, ctx }, dir)
    }

    /// Records a `tool.requested` event. `seq` fixes the call id's ULID
    /// ordinal so BTreeMap traversal order in `requested_calls` is
    /// deterministic across runs, not a race against wall-clock ULID ties.
    fn request(&self, seq: u128, invocation: InvocationId, provider_call_id: &str) -> ToolCall {
        let call = ToolCall {
            id: ToolCallId::from_ulid(ulid::Ulid(seq)),
            provider_call_id: provider_call_id.into(),
            name: "echo".into(),
            input: serde_json::Value::Null,
            agent_id: self.ctx.agent.id,
            task_id: None,
            invocation_id: Some(invocation),
            resources: Vec::new(),
            idempotency_key: None,
            requested_at: now(),
        };
        self.runtime
            .apply(
                Commit::new().event(self.ctx.draft(EventKind::ToolRequested {
                    call: call.clone(),
                    side_effect: aether_core::tool::SideEffectClass::ReadOnly,
                })),
            )
            .expect("record request");
        call
    }

    /// Settles a call with a successful, distinguishable outcome.
    fn settle(&self, call: ToolCallId, text: &str) {
        self.runtime
            .apply(
                Commit::new().event(self.ctx.draft(EventKind::ToolSucceeded {
                    call,
                    outcome: ToolOutcome::ok(text),
                })),
            )
            .expect("record outcome");
    }

    /// Records the assistant message a turn ends with: the tool uses the
    /// model asked for, on this invocation.
    fn assistant_tool_uses(&self, invocation: InvocationId, ids: &[&str]) {
        self.assistant_tool_uses_with(Some(invocation), ids);
    }

    /// Same, but for producers that stamp no invocation at all - the
    /// case the fix must never treat as a cross-invocation wildcard.
    fn assistant_tool_uses_without_invocation(&self, ids: &[&str]) {
        self.assistant_tool_uses_with(None, ids);
    }

    fn assistant_tool_uses_with(&self, invocation: Option<InvocationId>, ids: &[&str]) {
        let content = ids
            .iter()
            .map(|id| ContentBlock::ToolUse {
                id: (*id).into(),
                name: "echo".into(),
                input: serde_json::json!({}),
            })
            .collect();
        self.runtime
            .apply(Commit::new().event(self.ctx.draft(EventKind::AgentMessage {
                role: MessageRole::Assistant,
                content,
                invocation,
            })))
            .expect("record assistant message");
    }

    /// The `ToolResult` blocks of the message `compose_tool_results`
    /// appended, or `None` if it left the turn's calls unresolved.
    fn composed_results(&self) -> Option<Vec<ContentBlock>> {
        let events = self
            .runtime
            .store
            .latest_events(
                1,
                &EventFilter {
                    types: vec!["agent.message".into()],
                    ..EventFilter::own_events(self.ctx.agent.id)
                },
            )
            .expect("read events");
        match &events.first()?.kind {
            EventKind::AgentMessage {
                role: MessageRole::User,
                content,
                ..
            } if content
                .iter()
                .any(|b| matches!(b, ContentBlock::ToolResult { .. })) =>
            {
                Some(content.clone())
            }
            _ => None,
        }
    }
}
