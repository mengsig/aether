//! Subcommand implementations.

pub(crate) mod bridge;
pub(crate) mod github_watcher;
pub(crate) mod remote;
pub(crate) mod schedules;
pub(crate) mod voice;

mod memory;
pub use memory::{
    memory_forget, memory_list, memory_pin, memory_restore, memory_show, memory_sync,
};

#[cfg(test)]
use memory::{memory_scope_label, parse_memory_scope};

mod retention;
pub use retention::retention_preview;

mod outbox;
pub use outbox::{outbox_list, outbox_settle};

mod reports;
pub use reports::{replay, run_export, run_import, run_report, runs_list};

#[cfg(test)]
use reports::imported_lines;

mod diagnose;
pub use diagnose::diagnose;

mod diagnostics;
pub use diagnostics::doctor;

mod session;
#[cfg(test)]
use session::filter_known_models;
pub use session::tui;

mod execution;
pub use execution::run;

mod lifecycle;
#[cfg(test)]
use lifecycle::is_version_skew;
#[cfg(test)]
use lifecycle::record_startup_failure;
#[cfg(test)]
use lifecycle::report_startup_result;
#[cfg(test)]
use lifecycle::stop_unaskable_daemon;
pub use lifecycle::{daemon, serve, stop_endpoint};

mod providers;
use providers::health_segments;
pub use providers::{auth, login, logout, models, providers};

mod status;
pub use status::{mcp_bridge, mcp_list, mcp_serve, status};

mod agents;
pub use agents::{
    agents, archive, cancel, compact, interrupt, new_agent, pause, restart, resume, send, trigger,
    wake,
};

mod tasks;
#[cfg(test)]
use tasks::summarize;
pub use tasks::{events, task_create, tasks};

mod approvals;
pub use approvals::{approve, rules_add, rules_list, rules_revoke};

mod artifacts;
#[cfg(test)]
use artifacts::write_artifact_output;
pub use artifacts::{artifact_show, artifacts_list};

mod settings;
#[cfg(test)]
use settings::parse_approval;
#[cfg(test)]
use settings::parse_deadline;
#[cfg(test)]
use settings::parse_network;
#[cfg(test)]
use settings::parse_priority;
pub use settings::set;

mod update;
pub use update::{UpdateOptions, Version as ReleaseVersion, update};

use std::io;
use std::path::{Path, PathBuf};

use aether_client::{Client, ClientError, StartingDaemon, SubscriptionFrame, ensure_daemon};
use aether_core::agent::ArchiveTarget;
use aether_core::agent::{
    AgentConfig, ApprovalDefault, ApprovalPolicy, DeadlineUpdate, ExecutionBackend, NetworkPolicy,
    Scope,
};
use aether_core::approval::{RuleMatch, RuleScope};
use aether_core::artifact::ArtifactKind;
use aether_core::event::EventKind;
use aether_core::id::{AgentId, RunId};
use aether_core::model::{Effort, ModelChain};
use aether_core::provider::ProviderHealth;
use aether_core::task::Priority;
use aether_core::tool::ApprovalDecision;
use aether_daemon::DaemonConfig;
fn command_hint(endpoint: impl Into<aether_client::Endpoint>, explicit: bool) -> String {
    endpoint.into().command_hint(explicit)
}
use aether_protocol::{
    AgentQuery, ArtifactQuery, CreateAgent, Delta, EventQuery, MemoryQuerySpec, NewApprovalRule,
    RememberSpec, TaskQuery,
};
use aether_tui::TuiOptions;
use aether_tui::prefs::UiPrefs;
use aether_tui::ui::theme::ColorDepth;
use anyhow::{Context as _, anyhow, bail};
use tokio_util::sync::CancellationToken;

/// `aether daemon --foreground`, as this executable. The client detaches it
/// and watches it, so its startup output is not lost with its terminal.
fn daemon_command(
    socket: &PathBuf,
    yolo: bool,
    max_concurrent: Option<usize>,
    allow_remote: bool,
) -> std::io::Result<std::process::Command> {
    let exe = std::env::current_exe()?;
    let mut command = std::process::Command::new(exe);
    command
        .arg("--socket")
        .arg(socket)
        .arg("daemon")
        .arg("--foreground")
        .arg("--detached");
    if yolo {
        command.arg("--yolo");
    }
    if let Some(n) = max_concurrent {
        command.arg("--max-concurrent").arg(n.to_string());
    }
    // Without this the child fails `listen::check` on a non-loopback
    // `[listen] address` and dies at startup. That death is reported now,
    // but the daemon the caller asked for is still the one to start.
    if allow_remote {
        command.arg("--allow-remote");
    }
    Ok(command)
}

async fn connect_or_start(endpoint: impl Into<aether_client::Endpoint>) -> anyhow::Result<Client> {
    let endpoint = endpoint.into();
    if endpoint.is_remote() {
        return connect(&endpoint).await;
    }
    let socket = endpoint
        .local_path()
        .ok_or_else(|| anyhow!("missing local socket"))?
        .to_path_buf();
    let command = daemon_command(&socket, false, None, false)?;
    ensure_daemon(&socket, command).await.map_err(|e| {
        failed_connection(
            &socket,
            e,
            format!(
                "could not reach or start the daemon at {}",
                socket.display()
            ),
        )
    })
}

async fn connect(endpoint: impl Into<aether_client::Endpoint>) -> anyhow::Result<Client> {
    let endpoint = endpoint.into();
    let client = connect_unannounced(&endpoint).await?;
    outbox::announce_unconfirmed(&client, &endpoint).await?;
    Ok(client)
}

/// Connect without naming the operations still waiting on a decision, for the
/// commands that are about to show or settle them.
async fn connect_unannounced(endpoint: &aether_client::Endpoint) -> anyhow::Result<Client> {
    let client =
        Client::connect_endpoint(endpoint)
            .await
            .map_err(|error| match endpoint.local_path() {
                Some(path) => failed_connection(
                    path,
                    error,
                    format!(
                        "no daemon at {} (start one with `{}`)",
                        path.display(),
                        endpoint.command_hint(false)
                    ),
                ),
                None => anyhow::Error::new(error).context(format!(
                    "could not reach {}; check SSH access and the daemon on that host",
                    endpoint.display()
                )),
            })?;
    for (command, result) in client.recovered_commands() {
        match result {
            Ok(_) => eprintln!("confirmed queued remote {}", command.name()),
            Err(error) => eprintln!(
                "queued remote {} refused: {}",
                command.name(),
                sane(&error.to_string())
            ),
        }
    }
    Ok(client)
}

/// A connection that did not happen, said in terms of what to do next.
///
/// A daemon that refused this build is running, so neither starting one nor
/// looking for one is the way out - only stopping the one that holds the
/// socket is. Every other failure keeps `otherwise`, which is
/// the caller's own account of what it was trying to do. The client's error
/// stays the cause either way, so `stop` still recognises a refusal by type.
fn failed_connection(socket: &Path, error: ClientError, otherwise: String) -> anyhow::Error {
    let next = match &error {
        ClientError::Incompatible(_) => format!(
            "the daemon on {} is from another build; run `{} stop`, then retry",
            socket.display(),
            command_hint(socket, false)
        ),
        _ => otherwise,
    };
    anyhow::Error::new(error).context(next)
}

pub struct TuiDisplayOptions {
    pub view_mode: Option<String>,
    pub theme: Option<String>,
    pub reduced_motion: bool,
    pub plain: bool,
    pub ascii: bool,
}

/// The protocol schema on stdout, stable enough to diff: the generator in
/// `scripts/generate_clients.py` reads it and CI fails on drift.
pub fn schema() -> anyhow::Result<()> {
    println!(
        "{}",
        serde_json::to_string_pretty(&aether_protocol::schema::schema())?
    );
    Ok(())
}

/// What `aether run` was asked for, as the person wrote it.
#[derive(Debug)]
pub struct RunOptions {
    pub workspace: Option<PathBuf>,
    pub objective: String,
    /// `provider/model`; the configured default when absent.
    pub model: Option<String>,
    pub role: String,
    pub effort: Option<String>,
    /// Approve every tool call automatically.
    pub yes: bool,
    /// Approvals off (`--yolo`) or on (`--no-yolo`) for this agent. `None`
    /// leaves it to the daemon's `[approval] default`.
    pub yolo: Option<bool>,
    /// `host`, or `container:<image>`.
    pub backend: String,
    /// Override the daemon's network default for this root.
    pub network: Option<bool>,
    /// Print the outcome as one object rather than streaming the reply.
    pub json: bool,
}

/// A command that acts and then confirms. `--json` prints the fact as an
/// object so a caller parses one shape instead of a sentence.
fn acted(json: bool, line: &str, value: serde_json::Value) -> anyhow::Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(&value)?);
    } else {
        println!("{}", sane(line));
    }
    Ok(())
}

pub(crate) fn error_json(error: &anyhow::Error, socket: &Path) -> serde_json::Value {
    let code = if let Some(reply) = error.downcast_ref::<aether_protocol::ErrorReply>() {
        serde_json::json!(reply.code)
    } else if let Some(client) = error.downcast_ref::<ClientError>() {
        match client {
            ClientError::Protocol(reply) => serde_json::json!(reply.code),
            client if client.is_connection_failure() => serde_json::json!("daemon_unreachable"),
            ClientError::Incompatible(_) => serde_json::json!("incompatible"),
            ClientError::Unauthorized(_) => serde_json::json!("unauthorized"),
            ClientError::InvalidUsage(_) => serde_json::json!("invalid_argument"),
            ClientError::TimedOut { .. } => serde_json::json!("timed_out"),
            ClientError::DaemonStartup(_) => serde_json::json!("daemon_startup"),
            ClientError::Io(_) => serde_json::json!("io_error"),
            ClientError::Codec(_) | ClientError::Unexpected { .. } | ClientError::Malformed(_) => {
                serde_json::json!("protocol_error")
            }
            ClientError::Disconnected => serde_json::json!("daemon_unreachable"),
            ClientError::Ssh(_) => serde_json::json!("ssh_failure"),
            ClientError::DeliveryUnconfirmed(_) => serde_json::json!("delivery_unconfirmed"),
        }
    } else {
        serde_json::json!("internal")
    };
    serde_json::json!({"error": {
        "code": code,
        "message": format!("{error:#}"),
        "socket": socket.to_string_lossy(),
    }})
}

pub(crate) fn error_message(error: &anyhow::Error) -> String {
    let mut message = error.to_string();
    for cause in error.chain().skip(1) {
        let cause = cause.to_string();
        // Some wrappers include their cause in Display already.
        if !message.ends_with(&cause) {
            message.push_str(": ");
            message.push_str(&cause);
        }
    }
    sane(&message)
}

/// Text from somewhere else, with nothing in it that can move a cursor, set
/// a title or reorder what is read. Every run string the CLI prints comes
/// through this or through [`short`], which is this plus a width - and since
/// import, that string can come out of a file someone else wrote.
fn sane(text: &str) -> String {
    aether_core::text::strip_controls(text).into_owned()
}

/// Whole documents keep indentation when displayed or piped to another program.
fn sane_document(text: &str) -> String {
    let mut clean = String::with_capacity(text.len());
    for (index, part) in text.split('\t').enumerate() {
        if index != 0 {
            clean.push('\t');
        }
        clean.push_str(&aether_core::text::strip_controls(part));
    }
    clean
}

/// [`sane`] on one line, cut to `max` characters. For anywhere the line has
/// a width to keep; where the whole string is the point, `sane` alone.
fn short(text: &str, max: usize) -> String {
    let t = sane(text).replace('\n', " ");
    if t.chars().count() <= max {
        t
    } else {
        format!("{}…", t.chars().take(max).collect::<String>())
    }
}

/// Where a rule applies, from the mutually exclusive CLI flags.
pub struct RuleScopeArgs {
    pub agent: Option<String>,
    pub subtree: Option<String>,
    pub run: Option<String>,
    pub everywhere: bool,
}

impl RuleScopeArgs {
    /// `None` when no flag was given, so a caller can pick its own default.
    pub fn parse(&self) -> anyhow::Result<Option<RuleScope>> {
        let given = [
            self.agent.is_some(),
            self.subtree.is_some(),
            self.run.is_some(),
            self.everywhere,
        ]
        .iter()
        .filter(|g| **g)
        .count();
        if given > 1 {
            bail!("give at most one of --agent, --subtree, --run, --everywhere");
        }
        if let Some(a) = &self.agent {
            return Ok(Some(RuleScope::Agent {
                id: a.parse().context("invalid agent id")?,
            }));
        }
        if let Some(a) = &self.subtree {
            return Ok(Some(RuleScope::Subtree {
                root: a.parse().context("invalid agent id")?,
            }));
        }
        if let Some(r) = &self.run {
            return Ok(Some(RuleScope::Run {
                id: r.parse().context("invalid run id")?,
            }));
        }
        Ok(self.everywhere.then_some(RuleScope::Everywhere))
    }
}

enum ArtifactContents {
    Dropped { at: String },
    Retained { text: Vec<u8>, truncated: bool },
}

/// The directory the person is in: a root agent works there, whatever
/// directory the daemon happened to start in.
fn here(backend: ExecutionBackend, network: NetworkPolicy) -> anyhow::Result<Scope> {
    Ok(Scope {
        workspace_root: Some(std::env::current_dir().context("read the launch directory")?),
        backend,
        network,
        ..Default::default()
    })
}

/// Every VISION §3.1 knob, settable on a live agent. One definition, used
/// both as the command-line surface and as this module's input. Each set
/// field becomes one `agent.config_updated` event; unset ones are left alone.
#[derive(Debug, clap::Args)]
pub struct SetKnobs {
    /// Model as provider/model, or a fallback chain provider/model,provider/model.
    #[arg(long)]
    pub model: Option<String>,
    /// Reasoning effort: low, medium, high, xhigh, max.
    #[arg(long)]
    pub effort: Option<String>,
    #[arg(long)]
    pub instructions: Option<String>,
    #[arg(long)]
    pub max_turns: Option<u32>,
    #[arg(long)]
    pub token_budget: Option<u64>,
    #[arg(long)]
    pub cost_budget: Option<f64>,
    /// Wall-clock stop, RFC 3339; a later instant extends an existing one,
    /// and `none` removes it.
    #[arg(long)]
    pub deadline: Option<String>,
    /// Expose only these tools; repeat for several.
    #[arg(long = "tool")]
    pub tools: Vec<String>,
    /// Expose everything but these; repeat for several.
    #[arg(long = "deny-tool")]
    pub deny_tools: Vec<String>,
    /// `none`, or side-effect classes needing a person, comma separated.
    #[arg(long)]
    pub approval: Option<String>,
    /// low, normal, high or critical.
    #[arg(long)]
    pub priority: Option<String>,
    /// Stay alive after the current task and wait for more work.
    #[arg(long)]
    pub long_lived: Option<bool>,
    /// Descendants running at once; 0 clears the cap.
    #[arg(long)]
    pub concurrency: Option<usize>,
    /// host or container:<image>.
    #[arg(long)]
    pub backend: Option<String>,
    /// denied or allowed.
    #[arg(long)]
    pub network: Option<String>,
}

/// `host`, or `container:<image>`.
fn parse_backend(spec: &str) -> anyhow::Result<ExecutionBackend> {
    spec.parse().map_err(|e: String| anyhow!(e))
}

// ---------------------------------------------------------------------------
// Runs
// ---------------------------------------------------------------------------

/// How a run report is asked for and where it goes.
pub struct ReportOptions {
    pub run: String,
    pub json: bool,
    pub md: bool,
    /// Write to this file instead of stdout.
    pub out: Option<PathBuf>,
}

// ----- memory: browse and curate ------------------------------------------

#[cfg(test)]
mod tests;
