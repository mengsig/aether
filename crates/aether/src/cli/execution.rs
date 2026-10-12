use super::session::validated_model_chain;
use super::{
    AgentConfig, ApprovalPolicy, CreateAgent, Delta, Effort, EventKind, EventQuery, ModelChain,
    NetworkPolicy, PathBuf, RunOptions, SubscriptionFrame, UiPrefs, connect_or_start, here,
    parse_backend, sane, sane_document, short,
};
use anyhow::{anyhow, bail};
use std::io::Write as _;

pub async fn run(
    socket: impl Into<aether_client::Endpoint>,
    explicit_socket: bool,
    options: RunOptions,
) -> anyhow::Result<i32> {
    let socket = socket.into();
    let command = super::command_hint(&socket, explicit_socket);
    let RunOptions {
        workspace,
        objective,
        model,
        role,
        effort,
        yes,
        yolo,
        backend,
        network,
        json,
    } = options;
    if objective.trim().is_empty() {
        bail!("give an objective, e.g. `{command} run \"add a README\"`");
    }
    let _guard = crate::logging::init("aether.log", false)?;
    // A broken ui.toml must not stop `run`: say so and fall back to the
    // daemon's own defaults, same as the TUI (`cli::tui` above).
    anyhow::ensure!(
        !socket.is_remote() || workspace.is_some(),
        "remote run requires --workspace /path/on/host; choose a host workspace in the TUI with /workspace"
    );
    let mut client = connect_or_start(&socket).await?;
    let instance = client
        .welcome()
        .map(|w| w.instance_id.as_str())
        .unwrap_or("unknown");
    let prefs_dir = aether_tui::prefs::session_dir(&socket, instance, &aether_tui::prefs::dir());
    let prefs = match aether_tui::prefs::UiPrefs::load(&prefs_dir) {
        Ok(prefs) => prefs,
        Err(e) => {
            eprintln!("ignoring bad preferences: {}", super::sane(&e.to_string()));
            UiPrefs::default()
        }
    };
    let chain = match model {
        Some(m) => Some(
            ModelChain::parse(&m)
                .ok_or_else(|| anyhow!("model must be provider/model[,provider/model...]"))?,
        ),
        // Without --model, the session default the TUI last saved, then the
        // daemon's configured one, validated the same way the
        // TUI validates it.
        None => validated_model_chain(&client, prefs.model_chain()).await,
    };
    // `--yolo`/`--no-yolo` override for this run; otherwise the session
    // default the TUI last saved, exactly like `cli::tui`:
    // a saved `yolo = false` must gate `run` too, not only the TUI.
    let yolo = yolo.or(prefs.yolo);
    let effort = effort
        .map(|e| e.parse::<Effort>().map_err(|e| anyhow!(e)))
        .transpose()?;
    let backend = parse_backend(&backend)?;
    let network = match network {
        Some(true) => NetworkPolicy::Allowed,
        Some(false) => NetworkPolicy::Denied,
        None => client.status().await?.network_default,
    };
    let scope = run_scope(&client, &socket, workspace, backend, network).await?;
    // Capture the cursor before creation so a fast outcome can be replayed.
    let initial = client.subscribe(None, EventQuery::default(), false).await?;
    let mut last_seq = initial.last_seq();
    let agent = client
        .create_agent(CreateAgent {
            role,
            objective: Some(objective),
            model: chain.as_ref().map(|c| c.primary.clone()),
            fallbacks: chain.map(|c| c.fallbacks).unwrap_or_default(),
            scope: Some(scope),
            config: effort.map(|effort| AgentConfig {
                effort: Some(effort),
                ..Default::default()
            }),
            approval: yolo.map(|on| {
                if on {
                    ApprovalPolicy::none()
                } else {
                    ApprovalPolicy::default()
                }
            }),
            ..Default::default()
        })
        .await?;
    initial.unsubscribe().await?;
    let query = EventQuery {
        agent: Some(agent.id),
        ..Default::default()
    };
    let mut sub = client
        .subscribe(Some(last_seq), query.clone(), true)
        .await?;
    eprintln!(
        "agent {} ({}) on {}",
        agent.id,
        sane(&agent.name),
        sane(&agent.chain().to_string())
    );
    let mut out = std::io::stdout();
    let mut streaming = false;
    let mut outcome = None;
    loop {
        let next = if socket.is_remote() {
            tokio::select! { _ = tokio::signal::ctrl_c() => bail!("remote run monitoring interrupted; the agent remains on the host"), next = sub.next() => next }
        } else {
            sub.next().await
        };
        let frame = match next {
            Some(frame) => frame,
            None if socket.is_remote() => {
                finish_stream(&mut out, &mut streaming)?;
                let (connected, subscribed, snapshot) =
                    reconnect_run(&socket, &client, &query, last_seq).await?;
                client = connected;
                sub = subscribed;
                last_seq = sub.last_seq();
                if snapshot.is_some() {
                    outcome = snapshot;
                    break;
                }
                continue;
            }
            None => break,
        };
        if let SubscriptionFrame::Event(event) = &frame {
            last_seq = event.seq;
        }
        match frame {
            SubscriptionFrame::Lagged { capacity } => {
                finish_stream(&mut out, &mut streaming)?;
                eprintln!(
                    "agent subscription exceeded {capacity} queued frames; replaying events (streamed text may be incomplete)"
                );
                sub = client
                    .subscribe(Some(last_seq), query.clone(), true)
                    .await?;
            }
            SubscriptionFrame::Delta {
                agent: a, delta, ..
            } if a == agent.id => match delta {
                // Under --json stdout carries one object at the end, so the
                // reply is not streamed into the middle of it.
                Delta::Text { text } if !json => {
                    if !text.is_empty() {
                        streaming = !text.ends_with('\n');
                    }
                    out.write_all(sane_document(&text).as_bytes())?;
                    out.flush()?;
                }
                Delta::TurnFinished if streaming && !json => {
                    streaming = false;
                    writeln!(out)?;
                }
                _ => {}
            },
            SubscriptionFrame::Event(event) if event.agent_id == Some(agent.id) => {
                match &event.kind {
                    EventKind::ToolRequested { call, .. } => {
                        finish_stream(&mut out, &mut streaming)?;
                        eprintln!(
                            "▸ {} {}",
                            sane(&call.name),
                            short(&call.input.to_string(), 120)
                        )
                    }
                    EventKind::ToolSucceeded { outcome, .. } => {
                        let line = outcome.content.lines().next().unwrap_or("");
                        let marker = if outcome.is_error { "error: " } else { "" };
                        eprintln!("  {marker}{}", short(line, 160));
                    }
                    EventKind::ToolFailed { error, .. } => {
                        eprintln!("  failed: {}", short(error, 160))
                    }
                    EventKind::ApprovalRequested {
                        approval,
                        tool,
                        description,
                        ..
                    } => {
                        if yes {
                            client
                                .resolve_approval(
                                    *approval,
                                    aether_core::tool::ApprovalDecision::Approved,
                                    Some("auto".into()),
                                )
                                .await?;
                            eprintln!("approved {}: {}", short(tool, 40), short(description, 120));
                        } else {
                            eprintln!(
                                "approval needed for {}: {}\n  run: {command} approve {approval}",
                                short(tool, 40),
                                short(description, 120)
                            );
                        }
                    }
                    EventKind::QuestionAsked { question } => {
                        eprintln!(
                            "\n? {}\n  run: {command} send {} \"...\"",
                            sane(question),
                            agent.id
                        );
                    }
                    EventKind::AgentCompleted { summary } => {
                        outcome = Some("completed");
                        if let Some(s) = summary {
                            eprintln!("\ndone: {}", sane(s));
                        }
                        break;
                    }
                    EventKind::AgentFailed { error } => {
                        outcome = Some("failed");
                        eprintln!("\nfailed: {}", sane(error));
                        break;
                    }
                    EventKind::AgentCancelled { .. } => {
                        outcome = Some("cancelled");
                        eprintln!("\ncancelled");
                        break;
                    }
                    EventKind::AgentWaiting { condition } if agent_is_idle(condition) => {
                        outcome = Some("waiting");
                        eprintln!(
                            "\n(agent is waiting for input; continue in `{command}` or `{command} send {}`)",
                            agent.id
                        );
                        break;
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
    finish_stream(&mut out, &mut streaming)?;
    let outcome =
        outcome.ok_or_else(|| anyhow!("run subscription ended before an outcome arrived"))?;
    sub.unsubscribe().await?;
    tracing::debug!(agent = %agent.id, "run finished; fetching final usage");
    let final_agent = client.get_agent(agent.id).await?;
    tracing::debug!("final usage fetched");
    let u = final_agent.stats.usage;
    let tree = client.tree(Some(final_agent.run), true).await?;
    let node = tree
        .nodes
        .get(&agent.id)
        .ok_or_else(|| anyhow!("agent {} is missing from its run tree", agent.id))?;
    let subtree_usage = (!node.children.is_empty()).then_some(node.subtree_usage);
    eprintln!(
        "usage: {} in · {} cached · {} out · {} turns · {}",
        u.input_tokens,
        u.cache_read_tokens,
        u.output_tokens,
        final_agent.stats.turns,
        u.cost_and_estimate_line()
    );
    if let Some(total) = subtree_usage {
        eprintln!(
            "subtree usage: {} in · {} cached · {} out · {}",
            total.input_tokens,
            total.cache_read_tokens,
            total.output_tokens,
            total.cost_and_estimate_line()
        );
    }
    if json {
        let mut result = serde_json::json!({
            "agent": final_agent.id,
            "run": final_agent.run,
            "task": final_agent.current_task,
            "outcome": outcome,
            "status": final_agent.status,
            "turns": final_agent.stats.turns,
            "usage": u,
        });
        if let Some(total) = subtree_usage {
            result["subtree_usage"] = serde_json::to_value(total)?;
        }
        println!("{}", serde_json::to_string_pretty(&result)?);
    }
    Ok(match outcome {
        "failed" => 1,
        "cancelled" => 3,
        _ => 0,
    })
}

fn finish_stream(out: &mut impl std::io::Write, streaming: &mut bool) -> std::io::Result<()> {
    if *streaming {
        writeln!(out)?;
        out.flush()?;
        *streaming = false;
    }
    Ok(())
}

fn agent_is_idle(condition: &aether_core::agent::WaitCondition) -> bool {
    matches!(condition, aether_core::agent::WaitCondition::Inbox)
}

async fn run_scope(
    client: &aether_client::Client,
    endpoint: &aether_client::Endpoint,
    workspace: Option<PathBuf>,
    backend: aether_core::agent::ExecutionBackend,
    network: NetworkPolicy,
) -> anyhow::Result<aether_core::agent::Scope> {
    let Some(path) = workspace else {
        return here(backend, network);
    };
    let path = if endpoint.is_remote() {
        match client
            .request(aether_protocol::Command::ValidateWorkspace {
                path: path
                    .to_str()
                    .ok_or_else(|| anyhow!("workspace must be UTF-8"))?
                    .into(),
            })
            .await?
        {
            aether_protocol::Reply::WorkspaceValidation(validation) => {
                anyhow::ensure!(
                    validation.valid,
                    "{}",
                    validation
                        .reason
                        .as_deref()
                        .unwrap_or("invalid host workspace")
                );
                let canonical = validation
                    .path
                    .ok_or_else(|| anyhow!("host returned no canonical workspace"))?;
                anyhow::ensure!(canonical.is_absolute(), "host workspace is not absolute");
                canonical
            }
            other => bail!("expected host workspace validation, got {other:?}"),
        }
    } else {
        std::fs::canonicalize(path)?
    };
    Ok(aether_core::agent::Scope {
        workspace_root: Some(path),
        backend,
        network,
        ..Default::default()
    })
}

#[path = "run_reconnect.rs"]
mod run_reconnect;
use run_reconnect::reconnect_run;

async fn recovered_run_outcome(
    client: &aether_client::Client,
    query: &EventQuery,
) -> anyhow::Result<Option<&'static str>> {
    let agent = query
        .agent
        .ok_or_else(|| anyhow!("run reconnect has no agent"))?;
    let status = client.get_agent(agent).await?.status;
    Ok(match status {
        aether_core::agent::AgentStatus::Completed { .. } => Some("completed"),
        aether_core::agent::AgentStatus::Failed { .. } => Some("failed"),
        aether_core::agent::AgentStatus::Cancelled { .. } => Some("cancelled"),
        aether_core::agent::AgentStatus::Waiting { ref condition } if agent_is_idle(condition) => {
            Some("waiting")
        }
        _ => None,
    })
}
