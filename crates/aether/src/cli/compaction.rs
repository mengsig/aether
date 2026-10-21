//! `aether compact`: ask for a compaction and, unless told not to wait,
//! follow it to its outcome so the person sees what happened.
use super::{acted, connect, sane};
use aether_client::SubscriptionFrame;
use aether_core::agent::AgentStatusKind;
use aether_core::event::{CompactionTrigger, Event, EventKind};
use aether_core::id::{AgentId, EventId};
use aether_protocol::EventQuery;
use std::collections::HashSet;

/// Exit status when the compaction ran and failed.
const FAILED_EXIT: i32 = 1;

const WATCHED: [&str; 4] = [
    "compaction.requested",
    "compaction.completed",
    "compaction.native",
    "compaction.failed",
];

pub async fn compact(
    socket: impl Into<aether_client::Endpoint>,
    agent: super::AgentRef,
    json: bool,
    no_wait: bool,
) -> anyhow::Result<i32> {
    let socket = socket.into();
    let client = connect(&socket).await?;
    let id = agent.resolve(&client, &socket).await?;
    let follow_up = format!("{} events --agent {id} --follow", socket.command_hint(true));
    if no_wait {
        client.compact(id).await?;
        acted(
            json,
            &aether_core::event::compaction_request_notice(&follow_up),
            serde_json::json!({"agent": id, "compaction_requested": true,
                "outcome": "accepted", "follow_up": follow_up}),
        )?;
        return Ok(0);
    }
    // Subscribed before asking, so an outcome that lands at once is not missed.
    let cursor = client.status().await?.last_seq;
    let query = EventQuery {
        agent: Some(id),
        types: WATCHED.map(String::from).to_vec(),
        ..Default::default()
    };
    let mut sub = client.subscribe(Some(cursor), query, false).await?;
    let agent = client.get_agent(id).await?;
    // Requests made from the cursor on, this command's among them; an outcome
    // answers this command only when it names one of these as its cause.
    let mut answering: HashSet<EventId> = pending_handoff(&client, id, cursor)
        .await?
        .into_iter()
        .collect();
    client.compact(id).await?;
    if !json {
        eprintln!("{}", started_line(&sane(&agent.name), agent.status.kind()));
    }
    let started = std::time::Instant::now();
    while let Some(frame) = sub.next().await {
        match frame {
            SubscriptionFrame::Event(event) if event.agent_id != Some(id) => {}
            SubscriptionFrame::Event(event) => {
                if matches!(event.kind, EventKind::CompactionRequested { .. }) {
                    answering.insert(event.id);
                } else if is_outcome(&event)
                    && event
                        .causation
                        .is_some_and(|cause| answering.contains(&cause))
                {
                    return report(&event, started.elapsed(), json, &follow_up);
                }
            }
            SubscriptionFrame::Lagged { capacity } => anyhow::bail!(
                "the event stream fell {capacity} frames behind; run `{follow_up}` to see the outcome"
            ),
            _ => {}
        }
    }
    anyhow::bail!(
        "the daemon closed the event stream before the compaction finished; run `{follow_up}` to see the outcome"
    )
}

fn started_line(name: &str, status: AgentStatusKind) -> String {
    let when = if status == AgentStatusKind::Running {
        " It starts as soon as the agent's current turn ends."
    } else {
        ""
    };
    format!(
        "Compacting {name}: summarizing older turns so the agent keeps working with room to spare. \
         This usually takes under a minute or two.{when} Ctrl-C stops watching; the compaction carries on."
    )
}

fn is_outcome(event: &Event) -> bool {
    matches!(
        event.kind,
        EventKind::CompactionCompleted { .. }
            | EventKind::CompactionNative { .. }
            | EventKind::CompactionFailed { .. }
    )
}

/// A handoff still owed at `cursor` takes this command's request in: the daemon
/// records no new one, and the handoff's fold answers both.
async fn pending_handoff(
    client: &aether_client::Client,
    id: AgentId,
    cursor: u64,
) -> anyhow::Result<Option<EventId>> {
    const PAGE: usize = 32;
    let mut before = cursor.saturating_add(1);
    loop {
        let query = EventQuery {
            before: Some(before),
            agent: Some(id),
            types: WATCHED.map(String::from).to_vec(),
            ..Default::default()
        };
        let page = client.events(0, PAGE, query).await?;
        debug_assert!(page.iter().all(|event| event.seq < before));
        // Children's events come too; only the agent's own latest one counts.
        if let Some(latest) = page.iter().find(|event| event.agent_id == Some(id)) {
            let handoff = matches!(
                &latest.kind,
                EventKind::CompactionRequested {
                    trigger: CompactionTrigger::Handoff { .. }
                }
            );
            return Ok(handoff.then_some(latest.id));
        }
        match page.last() {
            Some(oldest) if page.len() == PAGE => {
                debug_assert!(oldest.seq < before, "paging must move back");
                before = oldest.seq;
            }
            _ => return Ok(None),
        }
    }
}

/// Print the outcome and return the exit status: 0 folded, 1 failed.
fn report(
    event: &Event,
    elapsed: std::time::Duration,
    json: bool,
    follow_up: &str,
) -> anyhow::Result<i32> {
    debug_assert!(event.agent_id.is_some());
    let failed = matches!(event.kind, EventKind::CompactionFailed { .. });
    let line = if failed {
        format!(
            "{}. Nothing was lost: the agent keeps its full context and carries on. \
             Run `aether compact {agent}` again to retry, or `{follow_up}` to watch the agent.",
            super::tasks::summarize(&event.kind),
            agent = event.agent_id.map(|id| id.to_string()).unwrap_or_default(),
        )
    } else {
        format!(
            "{} in {}s.",
            super::tasks::summarize(&event.kind),
            elapsed.as_secs()
        )
    };
    let value = serde_json::json!({
        "agent": event.agent_id,
        "compaction_requested": true,
        "outcome": if failed { "failed" } else { "completed" },
        "seconds": elapsed.as_secs_f64(),
        "event": event,
    });
    acted(json, &line, value)?;
    Ok(if failed { FAILED_EXIT } else { 0 })
}
