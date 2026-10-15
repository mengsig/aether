//! Durable corrections for native engines that describe killed tools as denials.
use aether_core::agent::Agent;
use aether_core::event::{Actor, EventDraft, EventKind};
use aether_core::id::ToolCallId;
use aether_store::{Commit, EventFilter};

use crate::{Result, Runtime};

pub(crate) const NATIVE_DENIED: &str = "tool.native_denied";

pub(crate) const EVENT: &str = "tool.interrupted";

pub(crate) fn record(
    commit: &mut Commit,
    agent: &Agent,
    call: ToolCallId,
    tool: &str,
    input: &serde_json::Value,
    cause: &str,
) {
    let tool = aether_model::http::safe_provider_text(tool, 64);
    let cause = aether_model::http::safe_provider_text(cause, 1024);
    let short_input = aether_model::http::safe_provider_text(&input.to_string(), 200);
    let note = format!(
        "Your previous tool call {tool:?} ({short_input}) was interrupted by {cause} before it finished. \
         It was not denied. Re-run it if you still need it, after checking for partial side effects."
    );
    let draft = |kind| {
        EventDraft::new(Actor::System, kind)
            .agent(agent.id)
            .parent(agent.parent)
            .run(agent.run)
    };
    // The projection consumes the cause before the generic closing event.
    commit.push_event(draft(EventKind::Custom {
        name: EVENT.into(),
        payload: serde_json::json!({"call": call, "tool": tool, "cause": cause, "note": note}),
    }));
    commit.push_event(draft(EventKind::ToolCancelled { call }));
}

/// A retained native session does not read Aether's transcript. Carry corrections
/// until an assistant response proves that a subsequent input reached the model.
pub(crate) fn resume_notes(runtime: &Runtime, agent: aether_core::id::AgentId) -> Result<String> {
    let completed = runtime.store.latest_events(
        1,
        &EventFilter {
            types: vec!["invocation.completed".into()],
            ..EventFilter::own_events(agent)
        },
    )?;
    let mut after = 0;
    if let Some(EventKind::InvocationCompleted { snapshot, .. }) =
        completed.first().map(|event| &event.kind)
    {
        // An engine can answer its own synthetic rejection before exiting.
        // Only notes preceding that answer's input snapshot were delivered.
        let mut limit = 1;
        loop {
            let snapshots = runtime.store.latest_events(
                limit,
                &EventFilter {
                    types: vec!["snapshot.created".into()],
                    ..EventFilter::own_events(agent)
                },
            )?;
            if let Some(event) = snapshots.iter().find(|event| matches!(&event.kind, EventKind::SnapshotCreated { snapshot: id, .. } if id == snapshot)) {
                after = event.seq;
                break;
            }
            if snapshots.len() < limit {
                break;
            }
            limit = limit.saturating_mul(2);
        }
    }
    let filter = EventFilter {
        types: vec!["custom".into()],
        ..EventFilter::own_events(agent)
    };
    let mut notes = String::new();
    loop {
        let events = runtime.store.events(after, 500, &filter)?;
        let Some(last) = events.last() else { break };
        after = last.seq;
        for event in &events {
            if let EventKind::Custom { name, payload } = &event.kind
                && name == EVENT
                && let Some(note) = payload["note"].as_str()
            {
                notes.push('\n');
                notes.push_str(note);
            }
        }
        if events.len() < 500 {
            break;
        }
    }
    Ok(notes)
}

/// Native ids belong to the engine, while approvals and projections use Aether ids.
/// Never guess from a tool name: several calls of the same tool can be in flight.
pub(crate) fn native_call_id(
    runtime: &Runtime,
    agent: aether_core::id::AgentId,
    provider_id: Option<&str>,
) -> Result<Option<ToolCallId>> {
    let Some(provider_id) = provider_id else {
        return Ok(None);
    };
    let mut after = runtime
        .store
        .latest_events(
            1,
            &EventFilter {
                types: vec!["agent.started".into()],
                ..EventFilter::own_events(agent)
            },
        )?
        .first()
        .map_or(0, |event| event.seq);
    loop {
        let events = runtime.store.events(
            after,
            500,
            &EventFilter {
                types: vec!["tool.requested".into()],
                ..EventFilter::own_events(agent)
            },
        )?;
        let Some(last) = events.last() else {
            return Ok(None);
        };
        after = last.seq;
        if let Some(call) = events.iter().find_map(|event| match &event.kind {
            EventKind::ToolRequested { call, .. } if call.provider_call_id == provider_id => {
                Some(call.id)
            }
            _ => None,
        }) {
            return Ok(Some(call));
        }
        if events.len() < 500 {
            return Ok(None);
        }
    }
}

pub(crate) fn denial_reason(
    runtime: &Runtime,
    agent: aether_core::id::AgentId,
    call: ToolCallId,
) -> Result<Option<String>> {
    Ok(runtime
        .store
        .tool_events(agent, aether_store::ToolEventScope::Calls(&[call]))?
        .into_iter()
        .find_map(|event| match event.kind {
            EventKind::ToolDenied { reason, .. } => Some(reason),
            _ => None,
        }))
}

/// A bridge denial may precede the native tool announcement on stdout.
pub(crate) fn pending_native_denial(
    runtime: &Runtime,
    agent: aether_core::id::AgentId,
    provider_id: &str,
) -> Result<Option<String>> {
    let mut after = runtime
        .store
        .latest_events(
            1,
            &EventFilter {
                types: vec!["agent.started".into()],
                ..EventFilter::own_events(agent)
            },
        )?
        .first()
        .map_or(0, |event| event.seq);
    loop {
        let events = runtime.store.events(
            after,
            500,
            &EventFilter {
                types: vec!["custom".into()],
                ..EventFilter::own_events(agent)
            },
        )?;
        let Some(last) = events.last() else {
            return Ok(None);
        };
        after = last.seq;
        if let Some(reason) = events.iter().find_map(|event| match &event.kind {
            EventKind::Custom { name, payload }
                if name == NATIVE_DENIED && payload["provider_call_id"] == provider_id =>
            {
                payload["reason"].as_str().map(str::to_owned)
            }
            _ => None,
        }) {
            return Ok(Some(reason));
        }
        if events.len() < 500 {
            return Ok(None);
        }
    }
}
