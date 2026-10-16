use super::{AgentId, ApprovalDefault, PathBuf, connect, health_segments, sane};
use anyhow::Context as _;

pub async fn status(socket: impl Into<aether_client::Endpoint>, json: bool) -> anyhow::Result<()> {
    let socket = socket.into();
    let client = connect(&socket).await?;
    let s = client.status().await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&s)?);
        return Ok(());
    }
    println!(
        "aetherd {} (pid {}) instance {}",
        sane(&s.version),
        s.pid,
        sane(&s.instance_id)
    );
    println!("started  {}", s.started_at.to_rfc3339());
    if let Some(bridge) = &s.bridge {
        println!("bridge   {}", sane(bridge));
    }
    println!(
        "model    {}",
        s.default_model
            .map(|m| sane(&m.to_string()))
            .unwrap_or_else(|| "none configured".into())
    );
    println!(
        "agents   {} live · {} running · {} waiting · {} blocked ({} suspended) · {} paused",
        s.agents_live,
        s.agents_running,
        s.agents_waiting,
        s.agents_blocked,
        s.agents_suspended,
        s.agents_paused
    );
    println!(
        "tasks    {} active · {} approvals pending",
        s.tasks_active, s.pending_approvals
    );
    if let Some(low) = &s.low_disk {
        let gib = |bytes: u64| bytes as f64 / (1u64 << 30) as f64;
        println!(
            "disk     LOW · {:.1} GiB free on the filesystem of {}, floor {:.1} GiB, since {} · new turns wait",
            gib(low.available_bytes),
            sane(&low.path),
            gib(low.floor_bytes),
            low.since.to_rfc3339()
        );
    }
    // The engine cap and the run budget bound burn rate directly, so say
    // where the daemon stands against both. Their own line: the `engines`
    // line below lists what is installed, which is a different question.
    println!(
        "limits   {} turns global · engines {} · run budget {}",
        s.max_concurrent_turns
            .map(|n| n.to_string())
            .unwrap_or_else(|| "?".into()),
        match s.max_concurrent_engine_turns {
            Some(cap) => format!("{}/{cap}", s.engine_turns_running),
            None => format!("{} running · uncapped", s.engine_turns_running),
        },
        match s.run_budget_usd {
            // The run closest to the budget is the one about to be refused.
            Some(budget) => match s
                .runs
                .iter()
                .max_by(|a, b| { a.usage.cost_usd.total_cmp(&b.usage.cost_usd) })
            {
                Some(worst) =>
                    format!("{} of ${budget}, highest live run", worst.usage.cost_line()),
                None => format!("${budget}, no live run"),
            },
            None => "none".to_owned(),
        }
    );
    println!(
        "approval {}",
        if s.yolo {
            "yolo daemon-wide · approvals off for every agent, no per-agent override".to_owned()
        } else {
            format!(
                "new agents: {}{}",
                s.approval_default,
                if s.approval_default == ApprovalDefault::Yolo {
                    " (auto-approve; scope and backend are the boundary)"
                } else {
                    " (external side effects are gated)"
                }
            )
        }
    );
    println!(
        "usage    {} in · {} cached · {} out · {}",
        s.usage_total.input_tokens,
        s.usage_total.cache_read_tokens,
        s.usage_total.output_tokens,
        s.usage_total.cost_and_estimate_line()
    );
    println!(
        "engines  {}",
        if s.engines.is_empty() {
            "none (install claude or codex for subscription access)".to_owned()
        } else {
            sane(&s.engines.join(" · "))
        }
    );
    println!("mcp      {}", render_mcp_summary(&s.mcp));
    if !s.providers.is_empty() {
        let rows: Vec<String> = s
            .providers
            .iter()
            .map(|p| format!("{} {}", sane(&p.provider), health_segments(p).join(" · ")))
            .collect();
        println!("providers {}", rows.join("\n          "));
    }
    for r in &s.runs {
        println!(
            "run      {} · {} live of {} agents · {} spent · {} · {}",
            r.run,
            r.agents_live,
            r.agents,
            r.usage.cost_and_estimate_line(),
            r.usage.said(r.rate_usd_per_hour, |v| format!("${v:.2}/h")),
            r.usage
                .said(r.forecast_usd_next_hour, |v| format!("${v:.2} next hour"))
        );
    }
    for holding in &s.resource_leases {
        println!(
            "holding  {} · {}{}",
            holding.agent,
            sane(
                &holding
                    .keys
                    .iter()
                    .map(|k| k.0.as_str())
                    .collect::<Vec<_>>()
                    .join(" ")
            ),
            if holding.quarantined {
                " · quarantined, awaiting inspection"
            } else {
                ""
            }
        );
    }
    println!("events   {}", s.last_seq);
    Ok(())
}

/// One line for the status header: how many servers are usable, and what is
/// wrong with the rest.
fn render_mcp_summary(servers: &[aether_core::mcp::McpServerHealth]) -> String {
    use aether_core::mcp::McpServerState;
    if servers.is_empty() {
        return "none configured".to_owned();
    }
    let ready = servers
        .iter()
        .filter(|s| s.state == McpServerState::Ready)
        .count();
    let tools: usize = servers.iter().map(|s| s.tools.len()).sum();
    let mut line = format!("{ready}/{} ready · {tools} tools", servers.len());
    for failed in servers.iter().filter(|s| s.state != McpServerState::Ready) {
        line.push_str(&format!(
            " · {} {}",
            sane(&failed.name),
            sane(failed.error.as_deref().unwrap_or("not contacted"))
        ));
    }
    line
}

/// Configured MCP servers, optionally reconnecting first.
pub async fn mcp_list(
    socket: impl Into<aether_client::Endpoint>,
    refresh: bool,
    server: Option<String>,
    json: bool,
) -> anyhow::Result<()> {
    let socket = socket.into();
    use aether_core::mcp::McpServerState;
    let client = connect(&socket).await?;
    let servers = client.mcp_servers(refresh, server).await?;
    if json {
        println!("{}", serde_json::to_string(&servers)?);
        return Ok(());
    }
    if servers.is_empty() {
        println!("no MCP servers configured ([[mcp.servers]] in config.toml)");
        return Ok(());
    }
    for s in &servers {
        let state = match s.state {
            McpServerState::Ready => "ready",
            McpServerState::Failed => "failed",
            McpServerState::Unknown => "not contacted",
        };
        println!("{}  {}  {}", sane(&s.name), state, sane(&s.transport));
        if let Some(error) = &s.error {
            println!("  error: {}", sane(error));
        }
        for tool in &s.tools {
            let declared = if tool.declared { "" } else { " (inferred)" };
            println!(
                "  {}  {:?}{declared}",
                sane(&tool.qualified_name),
                tool.side_effect
            );
        }
    }
    Ok(())
}

/// Serve Aether's own surface over MCP. Stdio is the default; HTTP needs a
/// token, taken from the environment so it never lands in a config file.
pub async fn mcp_serve(
    socket: impl Into<aether_client::Endpoint>,
    http: Option<std::net::SocketAddr>,
    token_env: String,
    allow_remote: bool,
) -> anyhow::Result<()> {
    let socket = socket.into();
    let sink = aether_client::mcp::DaemonSink(connect(&socket).await?);
    let Some(addr) = http else {
        // stdout is the protocol channel: the log must go to a file.
        let _guard = crate::logging::init("aether-mcp.log", false)?;
        return aether_client::mcp::stdio::serve(
            &sink,
            tokio::io::BufReader::new(tokio::io::stdin()),
            tokio::io::stdout(),
        )
        .await;
    };
    // Over HTTP stdout is not the protocol channel, so the warnings this path
    // emits - a non-loopback bind, a refused connection - can be seen.
    let _guard = crate::logging::init("aether-mcp.log", true)?;
    let token = std::env::var(&token_env)
        .ok()
        .filter(|t| !t.trim().is_empty())
        .with_context(|| format!("{token_env} is unset; the MCP HTTP endpoint requires a token"))?;
    let shutdown = tokio_util::sync::CancellationToken::new();
    let stopping = shutdown.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            stopping.cancel();
        }
    });
    let config = aether_client::mcp::http::HttpConfig {
        addr,
        token,
        allow_remote,
    };
    aether_client::mcp::http::serve(std::sync::Arc::new(sink), config, shutdown, |bound| {
        println!("aether mcp serve listening on http://{bound}");
    })
    .await
}

pub async fn mcp_bridge(socket: PathBuf, agent: String, token: String) -> anyhow::Result<()> {
    let _guard = crate::logging::init("aether-bridge.log", false)?;
    let id: AgentId = agent.parse().context("invalid agent id")?;
    aether_engine::bridge::run(socket, id, token).await
}
