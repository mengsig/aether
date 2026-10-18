use super::{AgentQuery, ArchiveTarget, acted, connect, sane, short};
use anyhow::bail;
use std::path::PathBuf;

pub async fn new_agent(
    socket: impl Into<aether_client::Endpoint>,
    name: Option<String>,
    model: Option<String>,
    json: bool,
) -> anyhow::Result<()> {
    let socket = socket.into();
    let chain = model
        .map(|model| {
            super::ModelChain::parse(&model)
                .ok_or_else(|| anyhow::anyhow!("model must be provider/model[,provider/model...]"))
        })
        .transpose()?;
    let client = super::connect_or_start(&socket).await?;
    let agent = client
        .create_agent(super::CreateAgent {
            role: aether_core::agent::Role::ASSISTANT.into(),
            name,
            model: chain.as_ref().map(|c| c.primary.clone()),
            fallbacks: chain.map(|c| c.fallbacks).unwrap_or_default(),
            ..Default::default()
        })
        .await?;
    super::acted(
        json,
        &format!(
            "created taskless agent {}; use `aether task --owner {} OBJECTIVE`",
            agent.id, agent.id
        ),
        serde_json::to_value(&agent)?,
    )
}

pub async fn agents(
    socket: impl Into<aether_client::Endpoint>,
    all: bool,
    json: bool,
) -> anyhow::Result<()> {
    let socket = socket.into();
    let client = connect(&socket).await?;
    let rows = client
        .list_agents(AgentQuery {
            live_only: !all,
            // Cleared work is finished work, so only `--all` can reach it.
            include_archived: all,
            ..Default::default()
        })
        .await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }
    // Cache reads are their own column: folded into `in` they let a
    // cache-dominated total read as fresh input nobody had paid full price for.
    println!(
        "{:<32} {:<14} {:<28} {:<28} {:>8} {:>8} {:>8} task",
        "id", "name", "status", "model", "in", "cached", "out"
    );
    for a in rows {
        println!(
            "{:<32} {:<14} {:<28} {:<28} {:>8} {:>8} {:>8} {}",
            a.id.to_string(),
            short(&a.name, 14),
            agent_status_label(&a),
            short(&a.chain().to_string(), 28),
            a.stats.usage.input_tokens,
            a.stats.usage.cache_read_tokens,
            a.stats.usage.output_tokens,
            a.current_task.map(|t| t.to_string()).unwrap_or_default()
        );
    }
    Ok(())
}

/// A pause is orthogonal to the status: a paused agent mid-turn is still
/// running, and saying only "paused" would be a lie.
fn agent_status_label(a: &aether_core::agent::Agent) -> String {
    let kind = if matches!(
        &a.status,
        aether_core::agent::AgentStatus::Blocked {
            reason: aether_core::agent::BlockReason::Exhausted { .. }
        }
    ) {
        format!("{} · exhausted", a.status.kind())
    } else {
        a.status.kind().to_string()
    };
    if a.paused {
        format!("{kind} · paused")
    } else {
        kind
    }
}

/// Clear finished work out of the live view, or bring it back. The mark is
/// durable; nothing leaves the store, reports or replay.
pub async fn archive(
    socket: impl Into<aether_client::Endpoint>,
    target: Option<super::TargetRef>,
    all_finished: bool,
    undo: bool,
    json: bool,
) -> anyhow::Result<()> {
    let socket = socket.into();
    match (&target, all_finished) {
        (Some(_), true) => bail!("name an agent or a run, or --all-finished, not both"),
        (None, false) => bail!("usage: aether archive <agent|run> | --all-finished"),
        _ => {}
    }
    let client = connect(&socket).await?;
    let target = match target {
        Some(target) => target.resolve(&client, &socket).await?,
        None => ArchiveTarget::AllFinished,
    };
    let moved = client.archive(target, !undo).await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&moved)?);
        return Ok(());
    }
    let verb = if undo { "restored" } else { "cleared" };
    println!("{verb} {} agent(s)", moved.len());
    for a in &moved {
        println!("  {} {}", a.id, sane(&a.name));
    }
    Ok(())
}

pub async fn send(
    socket: impl Into<aether_client::Endpoint>,
    agent: super::AgentRef,
    text: Vec<String>,
    images: Vec<PathBuf>,
    subtree: bool,
    json: bool,
) -> anyhow::Result<()> {
    let socket = socket.into();
    let text = super::text_from_process_stdin(text, images.is_empty())?;
    if text.trim().is_empty() && images.is_empty() {
        bail!("nothing to send: give the text as an argument, or pipe it in or use `-` for stdin");
    }
    // Read before connecting: a bad file is refused without a daemon.
    let attachments = images
        .iter()
        .map(|path| aether_client::images::read_image_file(path))
        .collect::<Result<Vec<_>, _>>()?;
    let client = connect(&socket).await?;
    let id = agent.resolve(&client, &socket).await?;
    if !subtree {
        let message = client
            .send_message_with_images(id, text, attachments)
            .await?;
        return acted(
            json,
            &format!("sent {message}"),
            serde_json::json!({"message": message, "to": id}),
        );
    }
    let deliveries = client
        .send_message_subtree_with_images(id, text, attachments)
        .await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&deliveries)?);
        return Ok(());
    }
    for d in &deliveries {
        println!("sent {} to {}", d.message, d.agent);
    }
    Ok(())
}

pub async fn cancel(
    socket: impl Into<aether_client::Endpoint>,
    agent: super::AgentRef,
    json: bool,
) -> anyhow::Result<()> {
    let socket = socket.into();
    let client = connect(&socket).await?;
    let id = agent.resolve(&client, &socket).await?;
    client
        .cancel(id, Some("cancelled from the command line".into()))
        .await?;
    acted(
        json,
        &format!(
            "cancellation requested for {id} and its live descendants (the daemon does not return the affected IDs)"
        ),
        serde_json::json!({"agent": id, "cancelled": true, "subtree": true}),
    )
}

pub async fn interrupt(
    socket: impl Into<aether_client::Endpoint>,
    agent: super::AgentRef,
    json: bool,
) -> anyhow::Result<()> {
    let socket = socket.into();
    let client = connect(&socket).await?;
    let id = agent.resolve(&client, &socket).await?;
    let was_running = client.interrupt(id).await?;
    let line = if was_running {
        format!("interrupted {id}; it waits for the next message")
    } else {
        format!("{id} has no turn running")
    };
    acted(
        json,
        &line,
        serde_json::json!({"agent": id, "was_running": was_running}),
    )
}

pub async fn trigger(
    socket: impl Into<aether_client::Endpoint>,
    agent: super::AgentRef,
    source: String,
    payload: String,
    json: bool,
) -> anyhow::Result<()> {
    let socket = socket.into();
    super::input::validate_trigger_source(&source)?;
    let payload = super::input::parse_trigger_payload(&payload)?;
    let client = connect(&socket).await?;
    let id = agent.resolve(&client, &socket).await?;
    client.trigger(id, source.clone(), payload).await?;
    acted(
        json,
        &format!("delivered trigger from {source} to {id}"),
        serde_json::json!({"agent": id, "source": source, "delivered": true}),
    )
}

/// Recreate a failed or cancelled agent from its last checkpoint.
pub async fn restart(
    socket: impl Into<aether_client::Endpoint>,
    agent: super::AgentRef,
    json: bool,
) -> anyhow::Result<()> {
    let socket = socket.into();
    let client = connect(&socket).await?;
    let id = agent.resolve(&client, &socket).await?;
    let fresh = client.restart(id).await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&fresh)?);
        return Ok(());
    }
    println!("{} restarted as {} ({})", id, fresh.id, sane(&fresh.name));
    Ok(())
}

/// Wake a failed agent in place, keeping its history and context.
pub async fn wake(
    socket: impl Into<aether_client::Endpoint>,
    agent: super::AgentRef,
    json: bool,
) -> anyhow::Result<()> {
    let socket = socket.into();
    let client = connect(&socket).await?;
    let id = agent.resolve(&client, &socket).await?;
    let woken = client.wake(id).await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&woken)?);
        return Ok(());
    }
    println!(
        "{} ({}) woken: now {}",
        woken.id,
        sane(&woken.name),
        woken.status.kind()
    );
    Ok(())
}

/// Pause an agent, or its whole live subtree (§3.1). Prints what it moved
/// and, for anything mid-turn, that the turn finishes first.
pub async fn pause(
    socket: impl Into<aether_client::Endpoint>,
    agent: super::AgentRef,
    subtree: bool,
    json: bool,
) -> anyhow::Result<()> {
    let socket = socket.into();
    let client = connect(&socket).await?;
    let id = agent.resolve(&client, &socket).await?;
    let (agents, mid_turn) = client.pause(id, subtree).await?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "paused": agents,
                "mid_turn": mid_turn,
            }))?
        );
        return Ok(());
    }
    if agents.is_empty() {
        // The pause is old news; the turn it caught is not.
        if mid_turn.is_empty() {
            println!("already paused");
        } else {
            println!("already paused · {} still finishing a turn", mid_turn.len());
        }
        return Ok(());
    }
    for a in &agents {
        let effect = if mid_turn.contains(&a.id) {
            "after the turn it is running"
        } else {
            "no turns until resumed"
        };
        println!("paused {} ({}) · {effect}", a.id, sane(&a.name));
    }
    Ok(())
}

/// Lift a pause, or clear a wait or an inspection the operator owns (§3.1).
pub async fn resume(
    socket: impl Into<aether_client::Endpoint>,
    agent: super::AgentRef,
    subtree: bool,
    json: bool,
) -> anyhow::Result<()> {
    let socket = socket.into();
    let client = connect(&socket).await?;
    let id = agent.resolve(&client, &socket).await?;
    let agents = client.resume(id, subtree).await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&agents)?);
        return Ok(());
    }
    if agents.is_empty() {
        println!("nothing to resume");
    } else {
        for a in &agents {
            println!("resumed {} ({}) · {}", a.id, sane(&a.name), a.status.kind());
        }
    }
    Ok(())
}

pub async fn compact(
    socket: impl Into<aether_client::Endpoint>,
    agent: super::AgentRef,
    json: bool,
) -> anyhow::Result<()> {
    let socket = socket.into();
    let client = connect(&socket).await?;
    let id = agent.resolve(&client, &socket).await?;
    client.compact(id).await?;
    let follow_up = format!("{} events --agent {id} --follow", socket.command_hint(true));
    acted(
        json,
        &aether_core::event::compaction_request_notice(&follow_up),
        serde_json::json!({"agent": id, "compaction_requested": true,
            "outcome": "accepted", "follow_up": follow_up}),
    )
}
