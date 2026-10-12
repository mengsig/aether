//! Native engine budget stop.
use super::{
    AgentId, Arc, BTreeMap, BTreeSet, CancellationToken, Commit, ContentBlock, EngineOutcome,
    EngineRequest, EventKind, MessageRole, Park, PendingCall, Result, Runtime, StreamState,
    TurnCtx, Usage, json, mpsc, start_park,
};
use aether_engine::EngineInput;

/// A grant the agent has crossed: what to report, and the figure it crossed on.
pub(super) struct Crossed {
    pub(super) reason: String,
    pub(super) usage: Usage,
}

/// The engine's result when its own grant stopped it where it stood. The
/// reason travels separately, because a budget stop is not an engine failure.
/// A result the turn was already holding stands under it: its reconciled
/// total and cost are what the engine measured, and dropping them books the
/// crossing turn at whatever the stream happened to have reported.
pub(super) fn budget_outcome(reason: &str, held: Option<EngineOutcome>) -> EngineOutcome {
    EngineOutcome {
        is_error: true,
        error: Some(reason.to_owned()),
        ..held.unwrap_or_default()
    }
}

/// Stopping the engine kills its whole process group, the `mcp-bridge` child
/// that has to relay the refusal included. Give the reply that long to leave.
/// Bounded, and deliberately the only grace here: the engine-specific
/// graceful stops belong in the adapters and in the park.
pub(super) const REFUSAL_GRACE: std::time::Duration = std::time::Duration::from_millis(500);

/// Wind the engine down because its grant is gone, through the same park a
/// harness action uses: it is asked to stop in its own verb, the process
/// follows at `grace`, and the reap still keeps the usage it measured. A turn
/// already parked has had its grace - the grant is gone, so it stops now.
/// True once the process has been stopped.
#[allow(clippy::too_many_arguments)]
pub(super) fn stop_for_crossing(
    park: &mut Option<Park>,
    agent: AgentId,
    grace: std::time::Duration,
    background: &BTreeSet<String>,
    input: &mut Option<mpsc::Sender<EngineInput>>,
    stop: Option<&mpsc::Sender<std::time::Duration>>,
    cancel: &CancellationToken,
    fired: mpsc::Sender<()>,
) -> bool {
    match park.as_mut() {
        Some(parked) => parked.stop_now(cancel),
        None => {
            *park = Some(start_park(
                agent, grace, background, input, stop, cancel, fired, false,
            ));
        }
    }
    park.as_ref().is_some_and(Park::killed)
}

/// The refusal in the shape its caller expects: a permission prompt is
/// answered with its protocol's deny body, every other tool with plain text.
pub(super) fn refusal(tool: &str, reason: &str) -> (String, bool) {
    if tool == "permission" {
        let body = json!({ "behavior": "deny", "message": reason });
        return (body.to_string(), false);
    }
    (format!("stopped: {reason}"), true)
}

/// The engine-side call a bridge request is about: the tool's own name, and
/// the provider's call id when the request carries one. A permission prompt
/// arrives under the name `permission` while gating some other tool, whose
/// name - and, when the engine sends it, whose `tool_use_id` - is the payload.
pub(super) fn gated_call(req: &EngineRequest) -> (String, Option<String>) {
    if req.name != "permission" {
        return (req.name.clone(), None);
    }
    (
        req.input["tool_name"]
            .as_str()
            .unwrap_or(&req.name)
            .to_owned(),
        req.input["tool_use_id"].as_str().map(str::to_owned),
    )
}

/// Which announced call a refusal answered. The provider's id when the
/// request carried one; otherwise the only pending call of that name. Never a
/// guess between two of a name: closing the wrong one puts a `stopped:` result
/// on a call nobody refused, and the engine may have several in flight.
pub(super) fn refused_call_id(
    calls: &BTreeMap<String, PendingCall>,
    (tool, call_id): &(String, Option<String>),
) -> Option<String> {
    if let Some(id) = call_id {
        return calls.contains_key(id).then(|| id.clone());
    }
    let mut of_name = calls.iter().filter(|(_, call)| &call.tool == tool);
    match (of_name.next(), of_name.next()) {
        (Some((id, _)), None) => Some(id.clone()),
        _ => None,
    }
}

/// Close the tool call a refusal answered, so the transcript does not carry a
/// call that looks like it never ran. The engine is killed here, so its own
/// echo of the result never arrives.
pub(super) fn record_refused_call(
    runtime: &Arc<Runtime>,
    ctx: &TurnCtx,
    gated: &(String, Option<String>),
    reason: &str,
    state: &mut StreamState,
) -> Result<()> {
    let refused = refused_call_id(&state.calls, gated)
        .and_then(|id| state.calls.remove(&id).map(|call| (id, call)));
    // Nothing to close: the engine had not announced the call, or more than
    // one of its name is pending and the refused one is not identifiable.
    let Some((provider_id, call)) = refused else {
        tracing::debug!(agent = %ctx.agent.id, tool = %gated.0, "no single announced call to close for this refusal");
        return Ok(());
    };
    let content = format!("stopped: {reason}");
    runtime.apply(
        Commit::new()
            // Denied, not a failed success: anything counting tool outcomes
            // counts a refusal as a refusal.
            .event(ctx.draft(EventKind::ToolDenied {
                call: call.id,
                reason: content.clone(),
            }))
            .event(ctx.draft(EventKind::AgentMessage {
                role: MessageRole::User,
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: provider_id,
                    content: vec![aether_core::model::ToolResultBlock::Text { text: content }],
                    is_error: true,
                }],
                invocation: None,
            })),
    )?;
    Ok(())
}
