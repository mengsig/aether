use super::{AgentId, EventKind, EventQuery, SubscriptionFrame, TaskQuery, connect, short};
use aether_core::model::{ContentBlock, ToolResultBlock};
use anyhow::Context as _;

pub async fn tasks(socket: impl Into<aether_client::Endpoint>, json: bool) -> anyhow::Result<()> {
    let socket = socket.into();
    let client = connect(&socket).await?;
    let rows = client.list_tasks(TaskQuery::default()).await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }
    println!(
        "{:<32} {:<10} {:<32} {:<10} objective",
        "id", "status", "owner", "waits on"
    );
    for t in rows {
        println!(
            "{:<32} {:<10} {:<32} {:<10} {}",
            t.id.to_string(),
            t.status.kind().as_str(),
            t.owner.map(|o| o.to_string()).unwrap_or_default(),
            t.dependencies.len(),
            short(&t.objective, 80)
        );
    }
    Ok(())
}

/// Create a task, optionally owned by an agent and gated on other tasks.
pub async fn task_create(
    socket: impl Into<aether_client::Endpoint>,
    objective: String,
    owner: Option<String>,
    depends_on: Vec<String>,
    json: bool,
) -> anyhow::Result<()> {
    let socket = socket.into();
    use aether_core::id::TaskId;
    use aether_protocol::CreateTask;
    let client = connect(&socket).await?;
    let owner = owner
        .map(|a| a.parse::<AgentId>())
        .transpose()
        .context("invalid agent id")?;
    let dependencies = depends_on
        .iter()
        .map(|t| t.parse::<TaskId>())
        .collect::<Result<Vec<_>, _>>()
        .context("invalid task id")?;
    let task = client
        .create_task(CreateTask {
            objective,
            owner,
            dependencies,
            ..Default::default()
        })
        .await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&task)?);
        return Ok(());
    }
    println!("{} {}", task.id, task.status.kind());
    Ok(())
}

pub async fn events(
    socket: impl Into<aether_client::Endpoint>,
    follow: bool,
    after: u64,
    agent: Option<String>,
    json: bool,
) -> anyhow::Result<()> {
    let socket = socket.into();
    let client = connect(&socket).await?;
    let agent = agent
        .map(|a| a.parse::<AgentId>())
        .transpose()
        .context("invalid agent id")?;
    let query = EventQuery {
        agent,
        ..Default::default()
    };
    // One event per line either way, so `--follow --json` is a stream a
    // caller reads as it arrives rather than a document it waits for.
    let print = |e: &aether_core::event::Event| -> anyhow::Result<()> {
        if json {
            println!("{}", serde_json::to_string(e)?);
            return Ok(());
        }
        println!(
            "{:>7} {} {:<24} {} {}",
            e.seq,
            e.at.format("%H:%M:%S%.3f"),
            e.type_name(),
            e.agent_id.map(|a| a.to_string()).unwrap_or_default(),
            super::sane(&summarize(&e.kind))
        );
        Ok(())
    };
    if !follow {
        let mut cursor = after;
        loop {
            let page = client.events(cursor, 1000, query.clone()).await?;
            if page.is_empty() {
                break;
            }
            for e in page {
                anyhow::ensure!(e.seq > cursor, "event page did not advance past {cursor}");
                cursor = e.seq;
                print(&e)?;
            }
        }
        return Ok(());
    }
    let mut sub = client.subscribe(Some(after), query, false).await?;
    while let Some(frame) = sub.next().await {
        match frame {
            SubscriptionFrame::Event(e) => print(&e)?,
            SubscriptionFrame::Lagged { capacity } => anyhow::bail!(
                "event subscription exceeded {capacity} queued frames; resume with --after using the last sequence printed"
            ),
            SubscriptionFrame::Delta { .. } => {}
        }
    }
    Ok(())
}

pub(super) fn summarize(kind: &EventKind) -> String {
    match kind {
        EventKind::CompactionCompleted {
            before_tokens: Some(before),
            after_tokens: Some(after),
            ..
        } => format!("Compaction completed: {before} -> {after} tokens"),
        EventKind::CompactionCompleted { .. } => "Compaction completed".into(),
        EventKind::CompactionFailed { error, .. } => format!("Compaction failed: {error}"),
        EventKind::AgentMessage { role, content, .. } => {
            format!("{role:?}: {}", short(&message_summary(content), 100))
        }
        EventKind::ToolRequested { call, .. } => {
            format!("{} {}", call.name, short(&call.input.to_string(), 80))
        }
        EventKind::ToolSucceeded { outcome, .. } if outcome.is_error => {
            format!("error: {}", short(&outcome.content, 100))
        }
        EventKind::ToolSucceeded { outcome, .. } => short(&outcome.content, 100),
        EventKind::ToolReplayed { key, original, .. } => {
            format!("replayed {original} under {key}")
        }
        EventKind::ToolRetried {
            attempt, of, error, ..
        } => format!("attempt {attempt}/{of} failed: {}", short(error, 100)),
        EventKind::ToolFailed { error, .. } => short(error, 100),
        // A native engine's spend arrives as agent.usage_updated, not per message.
        EventKind::InvocationCompleted {
            usage, duration_ms, ..
        } if usage.total_tokens() == 0 => format!("{duration_ms} ms; usage in agent.usage_updated"),
        EventKind::InvocationCompleted {
            usage, duration_ms, ..
        } => {
            format!(
                "{} in / {} out in {duration_ms} ms",
                usage.input_tokens, usage.output_tokens
            )
        }
        EventKind::AgentUsageUpdated { turn_usage, .. } => format!(
            "{} in / {} out this turn",
            turn_usage.input_tokens, turn_usage.output_tokens
        ),
        EventKind::TaskCompleted { result } => short(&result.summary, 100),
        EventKind::AgentFailed { error } | EventKind::TaskFailed { error, .. } => short(error, 100),
        EventKind::AgentWaiting { condition } => format!("{condition:?}"),
        EventKind::QuestionAsked { question } => short(question, 100),
        EventKind::Custom { name, payload } => {
            format!("{} {}", short(name, 60), short(&payload.to_string(), 100))
        }
        _ => String::new(),
    }
}

fn message_summary(content: &[ContentBlock]) -> String {
    let parts: Vec<String> = content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.clone()),
            // The input follows as its own tool.requested event.
            ContentBlock::ToolUse { name, .. } => Some(format!("calls {name}")),
            ContentBlock::ToolResult {
                content, is_error, ..
            } => {
                let mut part = String::from(if *is_error {
                    "tool result error: "
                } else {
                    "tool result: "
                });
                for block in content {
                    match block {
                        ToolResultBlock::Text { text } => part.push_str(text),
                        ToolResultBlock::Image { .. } => part.push_str(" [image]"),
                    }
                }
                Some(part)
            }
            _ => None,
        })
        .collect();
    parts.join(" ")
}
