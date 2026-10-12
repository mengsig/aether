use super::{AgentId, PathBuf, ReportOptions, RunId, connect, short};
use anyhow::{Context as _, bail};
use std::io::Write as _;

/// Every run this daemon's store knows about, newest first.
pub async fn runs_list(
    socket: impl Into<aether_client::Endpoint>,
    limit: usize,
    json: bool,
) -> anyhow::Result<()> {
    let socket = socket.into();
    let client = connect(&socket).await?;
    let runs = client.runs(limit).await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&runs)?);
        return Ok(());
    }
    if runs.is_empty() {
        println!("no runs yet");
        return Ok(());
    }
    let costs: Vec<String> = runs.iter().map(|r| r.usage.cost_line()).collect();
    // Runs from "$0.0100" to "≥ $0.1036 (some unpriced)".
    let cost_width = costs
        .iter()
        .map(|cost| cost.chars().count())
        .max()
        .unwrap_or(0);
    println!(
        "{:<32} {:>6} {:>5} {:>cost_width$} {:<21} objective",
        "run", "agents", "live", "cost", "started"
    );
    for (r, cost) in runs.iter().zip(costs) {
        // A run someone sent you looks exactly like one this daemon ran,
        // unless the row says so.
        let objective = match r.agents_imported {
            0 => short(r.objective.as_deref().unwrap_or(""), 60),
            _ => format!(
                "[imported] {}",
                short(r.objective.as_deref().unwrap_or(""), 48)
            ),
        };
        println!(
            "{:<32} {:>6} {:>5} {:>cost_width$} {:<21} {}",
            r.run.to_string(),
            r.agents,
            r.agents_live,
            cost,
            r.started_at.format("%Y-%m-%d %H:%M:%SZ").to_string(),
            objective,
        );
    }
    Ok(())
}

/// What a run did, composed from its events.
pub async fn run_report(
    socket: impl Into<aether_client::Endpoint>,
    options: ReportOptions,
) -> anyhow::Result<()> {
    let socket = socket.into();
    let run = options
        .run
        .parse::<RunId>()
        .with_context(|| format!("invalid run id {:?}", options.run))?;
    let client = connect(&socket).await?;
    let report = client.run_report(run).await?;
    if report.body.events == 0 {
        bail!("run {run} has no events in this daemon's store");
    }
    let text = if options.json {
        serde_json::to_string_pretty(&report)? + "\n"
    } else if options.md {
        report.to_markdown()
    } else {
        report.digest()
    };
    match options.out {
        Some(path) => {
            // A report carries the run's messages, tool inputs and errors,
            // so it lands as private as the export does.
            aether_core::private_file::write(&path, text.as_bytes())
                .with_context(|| format!("writing {}", path.display()))?;
            // Under --json a caller is parsing stdout, so what lands there
            // is JSON too; the prose goes where prose belongs.
            if options.json {
                println!(
                    "{}",
                    serde_json::json!({ "path": path.display().to_string() })
                );
            } else {
                eprintln!("report written to {}", path.display());
            }
        }
        None if options.json => print!("{text}"),
        None => print!("{}", super::sane(&text)),
    }
    Ok(())
}

/// Write a finished run to one file, on the daemon's machine.
pub async fn run_export(
    socket: impl Into<aether_client::Endpoint>,
    run: String,
    out: PathBuf,
    json: bool,
) -> anyhow::Result<()> {
    let socket = socket.into();
    let run = run
        .parse::<RunId>()
        .with_context(|| format!("invalid run id {run:?}"))?;
    // The daemon opens the file, so a relative path must mean the same thing
    // here as it does there.
    let out = std::path::absolute(&out).with_context(|| format!("path {}", out.display()))?;
    let client = connect(&socket).await?;
    let s = client.export_run(run, out).await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&s)?);
        return Ok(());
    }
    println!(
        "exported {} to {} · {} events · {} agents · {} tasks · {} snapshots · {} artifacts ({} with bytes) · {} bytes",
        s.run,
        s.path.display(),
        s.events,
        s.agents,
        s.tasks,
        s.snapshots,
        s.artifacts,
        s.artifact_payloads,
        s.bytes_written
    );
    Ok(())
}

/// Recreate a run from an export, under fresh ids.
pub async fn run_import(
    socket: impl Into<aether_client::Endpoint>,
    file: PathBuf,
    json: bool,
) -> anyhow::Result<()> {
    let socket = socket.into();
    let file = std::path::absolute(&file).with_context(|| format!("path {}", file.display()))?;
    let client = connect(&socket).await?;
    let s = client.import_run(file).await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&s)?);
        return Ok(());
    }
    print!(
        "{}",
        imported_lines(&s, &super::command_hint(&socket, false))
    );
    Ok(())
}

/// What an import says it did. `runs import` is the one command whose whole
/// input is a file this machine did not write, and this is the first thing
/// the operator sees of it, so `source_instance` - a free string out of that
/// file - is stripped before it reaches the terminal. Split out to be tested
/// as a string rather than asserted in a doc comment.
pub(super) fn imported_lines(s: &aether_core::transfer::ImportSummary, command: &str) -> String {
    format!(
        "imported {} as {} · {} events · {} agents · {} tasks · {} artifacts ({} with bytes)\n\
         from instance {} exported {}\n\
         `{command} runs report {}` reads it (its agents need `{command} agents --all`)\n",
        s.source_run,
        s.run,
        s.events,
        s.agents,
        s.tasks,
        s.artifacts,
        s.artifact_payloads,
        short(&s.source_instance, 64),
        s.exported_at.to_rfc3339(),
        s.run,
    )
}

/// Step through an agent's turns as they happened.
pub async fn replay(
    socket: impl Into<aether_client::Endpoint>,
    agent: String,
    json: bool,
) -> anyhow::Result<()> {
    let socket = socket.into();
    let agent = agent
        .parse::<AgentId>()
        .with_context(|| format!("invalid agent id {agent:?}"))?;
    let client = connect(&socket).await?;
    client.get_agent(agent).await?;
    let frames = client.replay(agent).await?;
    if json {
        // One frame per line: a caller consumes them as they arrive rather
        // than waiting for a whole document.
        let mut out = std::io::stdout().lock();
        for frame in &frames {
            writeln!(out, "{}", serde_json::to_string(frame)?)?;
        }
        return Ok(());
    }
    if frames.is_empty() {
        println!("{agent} has taken no turns yet");
        return Ok(());
    }
    for frame in &frames {
        println!(
            "\n─── turn {} · {} · {} · {} in / {} out · {}",
            frame.turn,
            frame.started_at.format("%H:%M:%S"),
            frame.outcome.as_str(),
            frame.usage.input_tokens,
            frame.usage.output_tokens,
            frame.usage.cost_line(),
        );
        if let Some(context) = &frame.context {
            let layers: Vec<String> = context
                .layers
                .iter()
                .map(|l| format!("{} {}t", l.kind.as_str(), l.tokens))
                .collect();
            println!(
                "saw   {} tokens · {}",
                context.token_estimate,
                layers.join(" · ")
            );
        }
        for message in &frame.messages {
            println!(
                "{:<6}{}",
                role_label(message.role),
                short(&blocks(&message.content), 100)
            );
        }
        for tool in &frame.tools {
            println!(
                "tool  {} {} → {}",
                short(&tool.name, 40),
                short(&tool.input.to_string(), 60),
                short(&tool_result(&tool.outcome), 60)
            );
        }
        for approval in &frame.approvals {
            println!(
                "ask   {} {} → {}",
                short(&approval.tool, 40),
                short(&approval.description, 50),
                approval
                    .decision
                    .map(|d| d.as_str())
                    .unwrap_or("unanswered")
            );
        }
    }
    Ok(())
}

fn role_label(role: aether_core::model::MessageRole) -> &'static str {
    use aether_core::model::MessageRole as R;
    match role {
        R::User => "user",
        R::Assistant => "model",
    }
}

/// The text of a message's blocks, for one line of a terminal.
fn blocks(content: &[aether_core::model::ContentBlock]) -> String {
    use aether_core::model::ContentBlock as B;
    content
        .iter()
        .map(|b| match b {
            B::Text { text } => text.clone(),
            B::Thinking { text, .. } => format!("(thinking) {text}"),
            B::ToolUse { name, .. } => format!("[call {name}]"),
            B::ToolResult { tool_use_id, .. } => format!("[result {tool_use_id}]"),
            B::Image { media_type, .. } => format!("[image {media_type}]"),
            B::ImageRef(image) => image.label(),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn tool_result(result: &aether_core::replay::ToolResult) -> String {
    use aether_core::replay::ToolResult as T;
    match result {
        T::Pending => "pending".into(),
        T::Succeeded {
            content, is_error, ..
        } => {
            if *is_error {
                format!("error: {content}")
            } else {
                content.clone()
            }
        }
        T::Failed { error, .. } => format!("failed: {error}"),
        T::Denied { reason } => format!("denied: {reason}"),
        T::Cancelled => "cancelled".into(),
        T::Replayed { original } => format!("replayed from {original}"),
    }
}
