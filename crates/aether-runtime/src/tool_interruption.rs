//! Durable corrections for native engines that describe killed tools as denials.
use aether_core::agent::Agent;
use aether_core::event::{Actor, EventDraft, EventKind};
use aether_core::id::ToolCallId;
use aether_store::{Commit, EventFilter};

use crate::{Result, Runtime};

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
        payload: serde_json::json!({"call": call, "cause": cause, "note": note}),
    }));
    commit.push_event(draft(EventKind::ToolCancelled { call }));
}

/// A retained native session does not read Aether's transcript. Carry corrections
/// until an assistant response proves that a subsequent input reached the model.
pub(crate) fn resume_notes(runtime: &Runtime, agent: aether_core::id::AgentId) -> Result<String> {
    let after = runtime
        .store
        .latest_events(
            1,
            &EventFilter {
                types: vec!["invocation.completed".into()],
                ..EventFilter::own_events(agent)
            },
        )?
        .first()
        .map_or(0, |event| event.seq);
    let filter = EventFilter {
        types: vec!["custom".into()],
        ..EventFilter::own_events(agent)
    };
    let mut after = after;
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
