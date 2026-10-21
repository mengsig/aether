//! `aether compact`: ask for a compaction and, unless told not to wait,
//! follow it to its outcome so the person sees what happened.
use super::{acted, connect, sane};
use aether_client::SubscriptionFrame;
use aether_core::agent::AgentStatusKind;
use aether_core::event::{Event, EventKind};
use aether_core::id::AgentId;
use aether_protocol::EventQuery;

/// Exit status when the compaction ran and failed.
const FAILED_EXIT: i32 = 1;

const OUTCOMES: [&str; 3] = [
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
        types: OUTCOMES.map(String::from).to_vec(),
        ..Default::default()
    };
    let mut sub = client.subscribe(Some(cursor), query, false).await?;
    let agent = client.get_agent(id).await?;
    client.compact(id).await?;
    if !json {
        eprintln!("{}", started_line(&sane(&agent.name), agent.status.kind()));
    }
    let started = std::time::Instant::now();
    while let Some(frame) = sub.next().await {
        match frame {
            SubscriptionFrame::Event(event) if is_outcome(&event, id) => {
                if !still_owed(&client, id, event.seq).await? {
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

fn is_outcome(event: &Event, id: AgentId) -> bool {
    event.agent_id == Some(id)
        && matches!(
            event.kind,
            EventKind::CompactionCompleted { .. }
                | EventKind::CompactionNative { .. }
                | EventKind::CompactionFailed { .. }
        )
}

/// A compaction is still owed after an outcome when a request follows it: this
/// command's own, or one re-issued because the engine folded mid-turn.
async fn still_owed(
    client: &aether_client::Client,
    id: AgentId,
    outcome_seq: u64,
) -> anyhow::Result<bool> {
    const PAGE: usize = 64;
    let query = EventQuery {
        agent: Some(id),
        types: vec!["compaction.requested".into()],
        ..Default::default()
    };
    let mut after = outcome_seq;
    loop {
        let page = client.events(after, PAGE, query.clone()).await?;
        debug_assert!(page.iter().all(|event| event.seq > after));
        if page.iter().any(|event| event.agent_id == Some(id)) {
            return Ok(true);
        }
        match page.last() {
            Some(last) if page.len() == PAGE => {
                debug_assert!(last.seq > after, "paging must move forward");
                after = last.seq;
            }
            _ => return Ok(false),
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
