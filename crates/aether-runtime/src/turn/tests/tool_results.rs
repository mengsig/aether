use super::*;

fn tool_result(id: &str, text: &str) -> ContentBlock {
    ContentBlock::ToolResult {
        tool_use_id: id.into(),
        content: vec![ToolResultBlock::Text { text: text.into() }],
        is_error: false,
    }
}

/// Two model invocations that reuse the same `provider_call_id`
/// (`ScriptedClient` does this every response by design, and a real
/// provider's short per-turn index can too) must never let the newer
/// invocation's tool result bind to an older, already-settled call that
/// happens to share that id. Old code's `requested_calls().values().find`
/// walks a `BTreeMap<ToolCallId, _>` oldest-first and matches on
/// `provider_call_id` alone, so it always found the *older* call first.
#[tokio::test]
async fn compose_tool_results_never_binds_a_stale_call_with_a_reused_provider_id() {
    let (h, _dir) = ComposeHarness::new().await;
    let (old_inv, new_inv) = (InvocationId::new(), InvocationId::new());

    let old_call = h.request(1, old_inv, "call_0");
    h.settle(old_call.id, "stale");
    h.assistant_tool_uses(old_inv, &["call_0"]);

    let new_call = h.request(2, new_inv, "call_0");
    h.settle(new_call.id, "fresh");
    h.assistant_tool_uses(new_inv, &["call_0"]);

    compose_tool_results(&h.runtime, &h.ctx).expect("compose");
    let content = h.composed_results().expect("a tool result was composed");
    assert_eq!(
        content,
        vec![tool_result("call_0", "fresh")],
        "the current invocation's own settled result must win, never an older invocation's"
    );
}

/// The mirror case: the *older* call happens to be settled and the
/// current invocation's own call is still pending. Old code found the
/// older, unrelated settle via the same id and composed a premature,
/// wrong result instead of waiting; it must keep waiting instead.
#[tokio::test]
async fn compose_tool_results_keeps_waiting_when_only_an_older_reused_id_has_settled() {
    let (h, _dir) = ComposeHarness::new().await;
    let (old_inv, new_inv) = (InvocationId::new(), InvocationId::new());

    let old_call = h.request(1, old_inv, "call_0");
    h.settle(old_call.id, "stale");
    h.assistant_tool_uses(old_inv, &["call_0"]);

    // The current invocation's own call is still pending: nothing to compose.
    h.request(2, new_inv, "call_0");
    h.assistant_tool_uses(new_inv, &["call_0"]);

    compose_tool_results(&h.runtime, &h.ctx).expect("compose");
    assert!(
        h.composed_results().is_none(),
        "an older invocation's settle must never stand in for this invocation's own pending call"
    );
}

/// A multi-call message where one id is reused and one is new: every id
/// must resolve to its own invocation's call, not whichever call the
/// provider id happens to match first.
#[tokio::test]
async fn compose_tool_results_resolves_every_call_of_a_multi_call_message_to_its_own_invocation() {
    let (h, _dir) = ComposeHarness::new().await;
    let (old_inv, new_inv) = (InvocationId::new(), InvocationId::new());

    let old_call = h.request(1, old_inv, "call_0");
    h.settle(old_call.id, "stale");
    h.assistant_tool_uses(old_inv, &["call_0"]);

    let new_call_0 = h.request(2, new_inv, "call_0");
    let new_call_1 = h.request(3, new_inv, "call_1");
    h.settle(new_call_0.id, "first");
    h.settle(new_call_1.id, "second");
    h.assistant_tool_uses(new_inv, &["call_0", "call_1"]);

    compose_tool_results(&h.runtime, &h.ctx).expect("compose");
    let content = h.composed_results().expect("both calls settled");
    assert_eq!(
        content,
        vec![
            tool_result("call_0", "first"),
            tool_result("call_1", "second")
        ]
    );
}

/// Baseline: a single invocation with unique provider ids, the ordinary
/// case, still composes correctly once the fix scopes lookups to the
/// current invocation.
#[tokio::test]
async fn compose_tool_results_still_composes_the_ordinary_single_invocation_case() {
    let (h, _dir) = ComposeHarness::new().await;
    let invocation = InvocationId::new();
    let call = h.request(1, invocation, "call_0");
    h.settle(call.id, "ok");
    h.assistant_tool_uses(invocation, &["call_0"]);

    compose_tool_results(&h.runtime, &h.ctx).expect("compose");
    let content = h.composed_results().expect("a tool result was composed");
    assert_eq!(content, vec![tool_result("call_0", "ok")]);

    // Idempotence: composing again over the now-appended `ToolResult`
    // message must be a no-op, not a duplicate result - the function
    // only ever acts on the *last* agent message, which is now the
    // composed one (role `User`, not `Assistant`), so it returns early.
    compose_tool_results(&h.runtime, &h.ctx).expect("compose again");
    assert_eq!(
        h.composed_results().expect("still the one composed result"),
        content,
        "a second compose must not duplicate or alter the already-composed result"
    );
}

/// steward follow-up: an assistant message with no `invocation` at all
/// (e.g. a producer that never stamped one) must never be treated as a
/// wildcard matching any invocation's calls; it must leave the calls
/// unresolved rather than binding to whichever call shares the provider
/// id, stale or not.
#[tokio::test]
async fn compose_tool_results_never_treats_a_missing_invocation_as_a_wildcard() {
    let (h, _dir) = ComposeHarness::new().await;
    let invocation = InvocationId::new();

    let call = h.request(1, invocation, "call_0");
    h.settle(call.id, "settled");
    h.assistant_tool_uses_without_invocation(&["call_0"]);

    compose_tool_results(&h.runtime, &h.ctx).expect("compose");
    assert!(
        h.composed_results().is_none(),
        "a missing assistant invocation must never act as a cross-invocation wildcard"
    );
}

#[tokio::test]
async fn compose_tool_results_resolves_batches_beyond_both_history_windows() {
    let (h, _dir) = ComposeHarness::new().await;
    let invocation = InvocationId::new();
    h.runtime
        .apply(
            Commit::new().event(h.ctx.draft(EventKind::InvocationCompleted {
                invocation,
                snapshot: aether_core::id::SnapshotId::new(),
                model: h.ctx.agent.model.clone(),
                usage: aether_core::model::Usage::default(),
                stop_reason: None,
                duration_ms: 0,
            })),
        )
        .expect("invocation receipt precedes requests");
    let ids: Vec<_> = (0..501).map(|index| format!("call_{index}")).collect();
    let mut oldest = None;
    for (index, id) in ids.iter().enumerate() {
        let call = h.request(index as u128 + 1, invocation, id);
        oldest.get_or_insert(call.id);
        h.settle(call.id, id);
    }
    let ids_ref: Vec<_> = ids.iter().map(String::as_str).collect();
    h.assistant_tool_uses(invocation, &ids_ref);
    compose_tool_results(&h.runtime, &h.ctx).expect("compose large batch");
    assert_eq!(
        h.composed_results().expect("all results"),
        ids.iter().map(|id| tool_result(id, id)).collect::<Vec<_>>()
    );
    let events = h
        .runtime
        .store
        .tool_events(
            h.ctx.agent.id,
            aether_store::ToolEventScope::Calls(&[oldest.unwrap()]),
        )
        .expect("old approval call");
    assert_eq!(events.len(), 2);
    assert!(matches!(events[0].kind, EventKind::ToolSucceeded { .. }));
    h.runtime.shutdown().await;
}

/// An outcome stored before masking, or before its secret was known, is
/// masked when an interrupted turn's tool results are composed from it.
#[tokio::test]
async fn compose_tool_results_masks_a_settled_outcome_stored_raw() {
    let (h, _dir) = ComposeHarness::new().await;
    let invocation = InvocationId::new();
    let token = format!("ghp_{}", "A".repeat(36));
    let succeeded = h.request(1, invocation, "call_0");
    h.settle(succeeded.id, &format!("read {token}"));
    let failed = h.request(2, invocation, "call_1");
    h.runtime
        .apply(Commit::new().event(h.ctx.draft(EventKind::ToolFailed {
            call: failed.id,
            error: format!("refused {token}"),
            retryable: false,
            duration_ms: None,
        })))
        .expect("record failure");
    h.assistant_tool_uses(invocation, &["call_0", "call_1"]);

    compose_tool_results(&h.runtime, &h.ctx).expect("compose");
    let content = h.composed_results().expect("a tool result was composed");
    let text = serde_json::to_string(&content).expect("serializes");
    assert!(!text.contains(&token), "{text}");
    assert_eq!(
        text.matches("[REDACTED:github_token]").count(),
        2,
        "both the success and the failure are masked: {text}"
    );
}
