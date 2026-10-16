use super::units;
use super::{AgentId, ApprovalDefault, PathBuf, connect, health_segments, sane};
use aether_core::time::{Timestamp, now};
use anyhow::Context as _;

pub async fn status(
    socket: impl Into<aether_client::Endpoint>,
    explicit_socket: bool,
    json: bool,
) -> anyhow::Result<()> {
    let socket = socket.into();
    let client = connect(&socket).await?;
    let s = client.status().await?;
    let installed = env!("CARGO_PKG_VERSION");
    let local = LocalInstall {
        // Only a daemon on this machine runs a build of this installation.
        installed: socket.local_path().map(|_| installed),
        version: version_line(&socket, &s.version, installed, explicit_socket),
    };
    println!("{}", status_text(&s, &local, json, now())?);
    Ok(())
}

/// What this installation adds to a local daemon's status; empty for a
/// remote one, which runs a binary of its own.
#[derive(Default)]
struct LocalInstall<'a> {
    installed: Option<&'a str>,
    version: Option<String>,
}

/// Exact values for `--json`, scaled ones for a person.
fn status_text(
    s: &aether_protocol::DaemonStatus,
    local: &LocalInstall<'_>,
    json: bool,
    at: Timestamp,
) -> anyhow::Result<String> {
    debug_assert_eq!(
        local.installed.is_some(),
        local.version.is_some(),
        "a local daemon has both an installed version and a version line"
    );
    if json {
        let mut value = serde_json::to_value(s)?;
        if let Some(installed) = local.installed {
            value
                .as_object_mut()
                .context("the daemon status is not a JSON object")?
                .insert("installed_version".to_owned(), installed.into());
        }
        return Ok(serde_json::to_string_pretty(&value)?);
    }
    Ok(human_status(s, local.version.as_deref(), at).join("\n"))
}

/// The status a person reads: scaled units and one qualifier per line,
/// where `--json` keeps every exact value.
fn human_status(
    s: &aether_protocol::DaemonStatus,
    version: Option<&str>,
    at: Timestamp,
) -> Vec<String> {
    let mut out = Vec::new();
    out.push(format!(
        "aetherd {} (pid {}) instance {}",
        sane(&s.version),
        s.pid,
        sane(&s.instance_id)
    ));
    if let Some(versions) = version {
        out.push(format!("version  {}", sane(versions)));
    }
    out.push(format!("started  {}", s.started_at.to_rfc3339()));
    if let Some(bridge) = &s.bridge {
        out.push(format!("bridge   {}", sane(bridge)));
    }
    out.push(format!(
        "model    {}",
        s.default_model
            .as_ref()
            .map(|m| sane(&m.to_string()))
            .unwrap_or_else(|| "none configured".into())
    ));
    out.push(format!(
        "agents   {} live · {} running · {} waiting · {} blocked ({} suspended) · {} paused",
        s.agents_live,
        s.agents_running,
        s.agents_waiting,
        s.agents_blocked,
        s.agents_suspended,
        s.agents_paused
    ));
    out.push(format!(
        "tasks    {} active · {} approvals pending",
        s.tasks_active, s.pending_approvals
    ));
    if let Some(low) = &s.low_disk {
        let gib = |bytes: u64| bytes as f64 / (1u64 << 30) as f64;
        out.push(format!(
            "disk     LOW · {:.1} GiB free on the filesystem of {}, floor {:.1} GiB, since {} · new turns wait",
            gib(low.available_bytes),
            sane(&low.path),
            gib(low.floor_bytes),
            low.since.to_rfc3339()
        ));
    }
    out.push(limits_line(s));
    out.push(approval_line(s));
    out.extend(activity_lines(s, at));
    debug_assert!(out.iter().all(|line| !line.is_empty()));
    out
}

fn limits_line(s: &aether_protocol::DaemonStatus) -> String {
    // The engine cap and the run budget bound burn rate directly, so say
    // where the daemon stands against both. Their own line: the `engines`
    // line below lists what is installed, which is a different question.
    format!(
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
    )
}

fn approval_line(s: &aether_protocol::DaemonStatus) -> String {
    format!(
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
    )
}

/// What the daemon has spent and is running: usage, engines, runs, holdings.
fn activity_lines(s: &aether_protocol::DaemonStatus, at: Timestamp) -> Vec<String> {
    let mut out = Vec::new();
    out.push(format!(
        "usage    {} in · {} cached · {} out · {}",
        units::tokens(s.usage_total.input_tokens),
        units::tokens(s.usage_total.cache_read_tokens),
        units::tokens(s.usage_total.output_tokens),
        s.usage_total.cost_and_estimate_line()
    ));
    out.push(format!(
        "engines  {}",
        if s.engines.is_empty() {
            "none (install claude or codex for subscription access)".to_owned()
        } else {
            sane(&s.engines.join(" · "))
        }
    ));
    out.push(format!("mcp      {}", render_mcp_summary(&s.mcp)));
    if !s.providers.is_empty() {
        let rows: Vec<String> = s
            .providers
            .iter()
            .map(|p| format!("{} {}", sane(&p.provider), health_segments(p).join(" · ")))
            .collect();
        out.push(format!("providers {}", rows.join("\n          ")));
    }
    out.extend(s.runs.iter().map(|r| run_line(r, at)));
    for holding in &s.resource_leases {
        out.push(format!(
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
        ));
    }
    out.push(format!("events   {}", s.last_seq));
    out
}

/// One run's spend, then its recent rate and where that leads within the
/// hour. An idle run forecasts nothing, and the cost qualifier is said once.
fn run_line(r: &aether_core::budget::RunSummary, at: Timestamp) -> String {
    debug_assert!(r.agents_live <= r.agents, "live agents are among the run's");
    debug_assert!(
        r.rate_usd_per_hour >= 0.0,
        "a rate of spend is never negative"
    );
    let mut line = format!(
        "run      {} · {} live of {} agents · {} spent",
        r.run,
        r.agents_live,
        r.agents,
        r.usage.cost_and_estimate_line()
    );
    if r.rate_usd_per_hour > 0.0 {
        let by = (at + chrono::Duration::hours(1)).with_timezone(&chrono::Local);
        line.push_str(&format!(
            " · {approx} ${:.2}/h now · {approx} ${:.2} total by {}",
            r.rate_usd_per_hour,
            r.forecast_usd_next_hour,
            by.format("%H:%M"),
            approx = aether_core::model::Usage::APPROX,
        ));
    }
    line
}

/// The daemon's version beside this binary's, and what restarts it when they
/// differ. `None` for a remote daemon, which runs a binary of its own.
fn version_line(
    endpoint: &aether_client::Endpoint,
    running: &str,
    installed: &str,
    explicit_socket: bool,
) -> Option<String> {
    endpoint.local_path()?;
    let versions = crate::doctor::running_versus_installed(running, installed);
    if running == installed {
        return Some(versions);
    }
    let aether = endpoint.command_hint(explicit_socket);
    Some(format!(
        "{versions} · the daemon keeps its build until `{aether} stop`; `{aether}` then starts \
         {installed}"
    ))
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

#[cfg(test)]
mod tests {
    use super::*;
    use aether_core::id::RunId;
    use serde_json::json;

    /// A daemon that cached 5.98B tokens and runs two runs: one billing
    /// $12/h now with part of its cost unknown, one idle since long ago.
    fn fixture(at: Timestamp) -> aether_protocol::DaemonStatus {
        let usage = |cost: f64, complete: bool| {
            json!({
                "input_tokens": 1_234_567, "output_tokens": 89_012,
                "cache_read_tokens": 5_981_893_206_u64, "cache_write_tokens": 0,
                "cost_usd": cost, "cost_complete": complete,
                "cost_gap": if complete { None } else { Some("interrupted") },
            })
        };
        serde_json::from_value(json!({
            "instance_id": "i", "version": "1", "pid": 7, "started_at": at,
            "last_seq": 42, "agents_live": 2, "agents_running": 1,
            "agents_waiting": 1, "agents_blocked": 0, "tasks_active": 1,
            "pending_approvals": 0, "usage_total": usage(855.18, false),
            "runs": [
                {"run": RunId::new(), "agents": 3, "agents_live": 1,
                 "usage": usage(843.18, false), "started_at": at,
                 "rate_usd_per_hour": 12.0, "forecast_usd_next_hour": 855.18},
                {"run": RunId::new(), "agents": 1, "agents_live": 1,
                 "usage": usage(12.0, true), "started_at": at,
                 "rate_usd_per_hour": 0.0, "forecast_usd_next_hour": 12.0},
            ],
        }))
        .expect("the fixture is a valid status")
    }

    #[test]
    fn a_person_reads_scaled_tokens_and_one_qualifier_per_run() {
        let at = aether_core::time::now();
        let text = status_text(&fixture(at), &LocalInstall::default(), false, at).expect("renders");
        assert!(
            text.contains("usage    1.23M in · 5.98B cached · 89.0k out"),
            "{text}"
        );
        assert!(!text.contains("5981893206"), "{text}");
        let runs: Vec<&str> = text.lines().filter(|l| l.starts_with("run ")).collect();
        assert_eq!(runs.len(), 2, "{text}");
        let by = (at + chrono::Duration::hours(1)).with_timezone(&chrono::Local);
        assert!(
            runs[0].ends_with(&format!(
                "≥ $843.1800 (interrupted run, cost unknown) spent · ≈ $12.00/h now · ≈ $855.18 total by {}",
                by.format("%H:%M")
            )),
            "{}",
            runs[0]
        );
        assert_eq!(runs[0].matches("cost unknown").count(), 1, "{}", runs[0]);
        assert!(
            runs[1].ends_with("$12.0000 spent"),
            "an idle run forecasts nothing: {}",
            runs[1]
        );
    }

    #[test]
    fn json_keeps_every_exact_value() {
        let at = aether_core::time::now();
        let status = fixture(at);
        let text = status_text(&status, &LocalInstall::default(), true, at).expect("renders");
        let value: serde_json::Value = serde_json::from_str(&text).expect("valid JSON");
        assert_eq!(value, serde_json::to_value(&status).expect("serializes"));
        assert_eq!(value["usage_total"]["cache_read_tokens"], 5_981_893_206_u64);
        assert_eq!(value["usage_total"]["input_tokens"], 1_234_567);
        assert_eq!(value["runs"][0]["rate_usd_per_hour"], 12.0);
        assert_eq!(value["runs"][0]["forecast_usd_next_hour"], 855.18);
        assert_eq!(value["runs"][1]["rate_usd_per_hour"], 0.0);
    }

    #[test]
    fn a_local_daemon_names_the_installed_version_in_both_forms() {
        let at = aether_core::time::now();
        let status = fixture(at);
        let local = LocalInstall {
            installed: Some("2"),
            version: Some("running 1 · installed 2".to_owned()),
        };
        let text = status_text(&status, &local, false, at).expect("renders");
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[1], "version  running 1 · installed 2", "{text}");
        let json = status_text(&status, &local, true, at).expect("renders");
        let value: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        assert_eq!(value["installed_version"], "2");
        assert_eq!(value["version"], "1");
    }
}
