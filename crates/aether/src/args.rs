use clap::{Parser, Subcommand};
use std::path::PathBuf;

mod remote;
mod subcommands;
pub(crate) use remote::*;
pub(crate) use subcommands::*;

#[derive(Debug, Parser)]
#[command(
    name = "aether",
    version,
    about = "Provider-agnostic, event-driven agent harness",
    after_help = "For commands with --json, command errors print one object on stdout: {\"error\":{\"code\":\"...\",\"message\":\"...\",\"socket\":\"...\"}}. A human error also goes to stderr; exit codes are unchanged. Argument parsing errors retain Clap's usage output."
)]
pub(crate) struct Args {
    /// Unix socket of the daemon (default: $XDG_RUNTIME_DIR/aether/aetherd.sock).
    #[arg(long, global = true, env = "AETHER_SOCKET")]
    pub(crate) socket: Option<PathBuf>,
    /// Use another machine's daemon, by the name `aether remote add` gave it.
    #[arg(long, global = true, env = "AETHER_REMOTE", value_name = "NAME")]
    pub(crate) remote: Option<String>,
    /// Use this machine's daemon, whatever AETHER_REMOTE says.
    #[arg(long, global = true)]
    pub(crate) local: bool,
    /// TUI theme for this run: auto, dark, light, mono, a built-in such as
    /// nord or gruvbox-light, or a theme file's name. `/theme` saves one.
    #[arg(long, global = true, env = "AETHER_THEME")]
    pub(crate) theme: Option<String>,
    /// No pulsing or shimmer: what moves holds still. Also
    /// `AETHER_REDUCED_MOTION` or `reduced_motion` in ui.toml.
    #[arg(long, global = true)]
    pub(crate) reduced_motion: bool,
    /// Lines instead of a screen: no colour, no glyphs, no motion. For screen
    /// readers, logs and pipes. Assumed when stdout is not a terminal.
    #[arg(long, global = true)]
    pub(crate) plain: bool,
    /// ASCII glyphs and decoration, with the full interactive TUI. Also
    /// AETHER_ASCII or `ascii` in ui.toml; message text is unchanged.
    #[arg(long, global = true)]
    pub(crate) ascii: bool,
    /// Named transcript presentation from modes/ or a built-in.
    #[arg(long, global = true)]
    pub(crate) view_mode: Option<String>,
    #[command(subcommand)]
    pub(crate) command: Option<Cmd>,
}

// Parsed once, at startup: the size gap between variants costs nothing.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Subcommand)]
pub(crate) enum Cmd {
    /// Attach the terminal UI (starts the daemon if needed).
    Tui {
        /// Focus this agent on start.
        #[arg(long)]
        agent: Option<String>,
        /// Leave the mouse to the terminal for this run, whatever /mouse
        /// saved. Mouse capture is on by default.
        #[arg(long = "no-mouse", overrides_with = "mouse")]
        no_mouse: bool,
        /// Enable pane scrolling and drag-to-copy for this run.
        /// Shift+drag uses native selection; /mouse off disables capture.
        #[arg(long, overrides_with = "no_mouse")]
        mouse: bool,
        /// Start agents with approvals off (the shipped default), or with
        /// `--no-yolo` gate them. /approvals toggles it later. Without either, the
        /// saved preference and then `[approval] default` decide.
        #[arg(long, overrides_with = "no_yolo")]
        yolo: bool,
        #[arg(long = "no-yolo", overrides_with = "yolo")]
        no_yolo: bool,
    },
    /// Run the daemon.
    Daemon {
        /// Stay attached to the terminal and log to stderr.
        #[arg(long)]
        foreground: bool,
        /// Set by the client that started this daemon: it has no terminal, so
        /// a failure goes to the startup log beside the socket. A flag rather
        /// than an environment variable, which every child would inherit.
        #[arg(long, hide = true, requires = "foreground")]
        detached: bool,
        /// Workspace root for new root agents (default: current directory).
        #[arg(long)]
        workspace: Option<PathBuf>,
        /// Maximum concurrent model turns (config value, otherwise 32).
        /// A person's message always gets one more.
        #[arg(long)]
        max_concurrent: Option<usize>,
        /// Skip every approval for every agent (dangerous): tool calls run,
        /// engines run unsandboxed. Also `[approval] yolo = true` in config.toml.
        #[arg(long)]
        yolo: bool,
        /// Let the `[listen]` TCP transport bind outside loopback. Aether
        /// terminates no TLS, so this means accepting plaintext on the
        /// network; an SSH tunnel to loopback is the safe way in.
        #[arg(long)]
        allow_remote: bool,
    },
    /// Run the `[serve]` doors: webhooks in, event subscriptions out.
    Serve {
        /// Bind the inbound listener outside loopback. Aether terminates no
        /// TLS, so this accepts plaintext hooks from the network.
        #[arg(long)]
        allow_remote: bool,
    },
    /// Print the protocol as a JSON Schema. The client libraries under
    /// `clients/` are generated from exactly this document.
    Schema,
    /// Inspect storage usage, or collect eligible files.
    Storage {
        #[command(subcommand)]
        command: Option<StorageCmd>,
        #[arg(long)]
        json: bool,
    },
    /// Review an agent's workspace changes, then land or throw them away.
    Workspace {
        #[command(subcommand)]
        command: WorkspaceCmd,
    },
    /// Inspect retention without deleting history or artifact payloads.
    Retention {
        #[command(subcommand)]
        command: RetentionCmd,
    },
    /// Decide remote operations whose delivery the host could not confirm.
    Outbox {
        #[command(subcommand)]
        command: Option<OutboxCmd>,
    },
    /// Show daemon status.
    Status {
        /// Print the whole status as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Stop the daemon. Agents resume when it starts again; a turn with uncertain
    /// tool effects waits for `aether resume AGENT_ID`.
    Stop {
        #[arg(long)]
        json: bool,
    },
    /// Create an agent with an objective and stream its work to stdout.
    #[command(
        after_help = "Exit codes: 0 = completed or waiting for operator input; 1 = failed run or command error; 2 = invalid command-line arguments; 3 = cancelled run. Usage and --json output are printed before the run's exit status is returned. Delegated runs also show subtree usage, including descendants."
    )]
    Run {
        /// Workspace on the selected host; required for --remote.
        #[arg(long)]
        workspace: Option<PathBuf>,
        /// The objective, as one argument or several words.
        objective: Vec<String>,
        /// Model as provider/model, or a fallback chain:
        /// `provider/model,provider/model` (default: the configured default).
        #[arg(long)]
        model: Option<String>,
        /// Role label for the agent. It is a root a person started, so it
        /// coordinates its own tree by default.
        #[arg(long, default_value = aether_core::agent::Role::ASSISTANT)]
        role: String,
        /// Reasoning effort: low, medium, high, xhigh, max.
        #[arg(long)]
        effort: Option<String>,
        /// Approve every tool call automatically.
        #[arg(long, short = 'y')]
        yes: bool,
        /// Approvals off: nothing asks first (the shipped default).
        #[arg(long, overrides_with = "no_yolo")]
        yolo: bool,
        /// Require approval for external side effects only; local file writes
        /// do not prompt. `run` starts work at once, so to also gate local
        /// mutations create the agent with /new in `aether tui`, apply `aether set
        /// <agent> --approval idempotent_mutation,mutation,external`, then send it work.
        #[arg(long = "no-yolo", overrides_with = "yolo")]
        no_yolo: bool,
        /// Where its tools run: `host`, or `container:<image>`.
        #[arg(long, default_value = "host")]
        backend: String,
        /// Override the configured network default; --network=false denies access.
        #[arg(long, num_args = 0..=1, default_missing_value = "true", require_equals = true)]
        network: Option<bool>,
        /// A checkout whose repository names the agent's memory, when it
        /// works somewhere else, such as a worktree. Local daemons only.
        #[arg(long)]
        repository_root: Option<PathBuf>,
        /// Print the outcome as one object on stdout instead of streaming
        /// the reply; progress stays on stderr either way.
        #[arg(long)]
        json: bool,
    },
    /// Log in to a provider (API key, or OAuth where supported).
    Login {
        provider: String,
        /// Use an API key even if OAuth is available.
        #[arg(long)]
        api_key: bool,
        /// OAuth method when several exist, e.g. browser or device.
        #[arg(long)]
        method: Option<String>,
    },
    /// Remove stored credentials for a provider.
    Logout {
        provider: String,
        #[arg(long)]
        json: bool,
    },
    /// Show credential status per provider.
    Auth {
        #[arg(long)]
        json: bool,
    },
    /// Check the installation: config, daemon, store, disk, providers,
    /// engines and MCP servers. Exit code 0 all well, 1 something to watch,
    /// 2 something broken.
    Doctor {
        #[arg(long)]
        json: bool,
    },
    /// Replace this binary with the latest released one. Asks the network
    /// only when run; nothing checks for updates in the background.
    #[command(
        after_help = "Exit codes: 0 = updated, or nothing to do; 1 = error, with the installed binary left as it was; 2 = invalid command-line arguments. With --check: 0 = up to date, 10 = a newer release exists. --json prints the outcome as one object and no progress lines; the exit codes are the same.\n\nOnly a binary that install.sh installed in a directory you can write is replaced; for any other, the command that updates it is printed. A running daemon keeps the old build until `aether stop` and the next `aether`. Where a daemon that refuses a newer build cannot be signalled (macOS, and Linux before 6.5), a running daemon is not left behind: nothing is installed until `aether stop` has been run. Assets come from GitHub releases, or from AETHER_INSTALL_BASE_URL, and their SHA-256 is checked before anything is replaced."
    )]
    Update {
        /// Only report whether a newer release exists (see the exit codes).
        #[arg(long, conflicts_with = "version")]
        check: bool,
        /// Install this release instead of the latest, e.g. v1.2.3. An
        /// older one is a downgrade, and is said to be one.
        #[arg(long, value_name = "TAG", value_parser = cli::ReleaseVersion::parse)]
        version: Option<cli::ReleaseVersion>,
        /// Print the outcome as one JSON object, with no progress lines.
        #[arg(long)]
        json: bool,
    },
    /// Write a private, redacted diagnostic directory (never starts a daemon).
    Diagnose {
        /// New directory to create; existing destinations are refused.
        #[arg(long)]
        out: PathBuf,
        /// Collect this run instead of the most recent runs.
        #[arg(long, conflicts_with = "last")]
        run: Option<aether_core::id::RunId>,
        /// Number of recent runs (1..=100).
        #[arg(long, default_value = "5", value_parser = clap::value_parser!(u32).range(1..=100))]
        last: u32,
        /// Print the bundle path and manifest as JSON.
        #[arg(long)]
        json: bool,
    },
    /// List models and whether their provider is configured.
    #[command(
        after_help = "Budgets: API providers accept token and turn limits, and dollar limits when a model price is configured (cost is estimated from list prices). Claude Code accepts token, turn and dollar limits (list-price estimates or provider-reported cost). Codex accepts token and turn limits; dollar limits are refused because it reports no enforceable dollar cost. Subscription API-equivalent estimates do not enforce dollar budgets."
    )]
    Models {
        #[arg(long)]
        json: bool,
    },
    /// List providers, their auth and their live health.
    Providers {
        #[arg(long)]
        json: bool,
    },
    /// Runs: what they did, and moving them between daemons.
    Runs {
        #[command(subcommand)]
        command: RunsCmd,
    },
    /// List agents.
    Agents {
        #[arg(long)]
        all: bool,
        #[arg(long)]
        json: bool,
    },
    /// Clear finished agents or runs out of the live view (they stay in the
    /// store, in reports and in replay), or bring them back with --undo.
    Archive {
        /// An agent id or a run id.
        target: Option<String>,
        /// Every finished agent there is - the same as `/clear` in the TUI.
        #[arg(long)]
        all_finished: bool,
        /// Bring the target back into the view instead.
        #[arg(long)]
        undo: bool,
        #[arg(long)]
        json: bool,
    },
    /// List tasks.
    Tasks {
        #[arg(long)]
        json: bool,
    },
    /// Create a task, optionally owned and gated on other tasks.
    #[command(
        after_help = "CLI example (requires jq):\n  owner=$(aether new --name planner --model codex/gpt-6.1-sol --json | jq -r .id)\n  aether task --owner \"$owner\" \"Review the README\"\n\nAn unowned task stays pending until an agent is assigned to it; creating it does not start work or automatically select an owner. An owned task becomes runnable when its dependencies have completed and the owner is ready and unpaused."
    )]
    Task {
        /// The objective, as one argument or several words.
        objective: Vec<String>,
        /// Live, taskless agent to own it. Agents from `aether run` already own
        /// a task. Create an eligible agent with `aether new` or TUI /new, then use its ID.
        #[arg(long)]
        owner: Option<String>,
        /// Task this one waits for; repeat for several.
        #[arg(long = "depends-on")]
        depends_on: Vec<String>,
        #[arg(long)]
        json: bool,
    },
    /// Create a taskless agent eligible to own a task. Uses daemon defaults
    /// for workspace, approval policy and model unless overridden here.
    New {
        #[arg(long)]
        name: Option<String>,
        /// Model as provider/model[,provider/model...] (default: daemon default).
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Step through an agent's turns as they happened: what it saw, what it
    /// did, what it cost.
    Replay {
        agent: String,
        /// One frame per line, for another process.
        #[arg(long)]
        json: bool,
    },
    /// Print events, optionally following live.
    Events {
        #[arg(long, short = 'f')]
        follow: bool,
        #[arg(long, default_value_t = 0)]
        after: u64,
        #[arg(long)]
        agent: Option<String>,
        /// One event per line, so `-f --json` is a stream.
        #[arg(long)]
        json: bool,
    },
    /// Send a message to an agent.
    Send {
        agent: String,
        text: Vec<String>,
        /// Attach an image file (PNG, JPEG, GIF or WebP); repeat for more.
        #[arg(long = "image", value_name = "FILE")]
        images: Vec<PathBuf>,
        /// Deliver to every live agent beneath it as well, each its own item.
        #[arg(long)]
        subtree: bool,
        #[arg(long)]
        json: bool,
    },
    /// List pending tool approvals.
    Approvals {
        #[command(flatten)]
        scope: ApprovalScopeArgs,
        /// Print pending requests as a JSON array.
        #[arg(long)]
        json: bool,
    },
    /// Approve a pending tool call.
    Approve {
        #[arg(
            required_unless_present = "all_pending",
            conflicts_with = "all_pending"
        )]
        approval: Option<String>,
        #[command(flatten)]
        remember: RememberArgs,
    },
    /// Deny a pending tool call.
    Deny {
        #[arg(
            required_unless_present = "all_pending",
            conflicts_with = "all_pending"
        )]
        approval: Option<String>,
        #[command(flatten)]
        remember: RememberArgs,
    },
    /// Artifacts: large outputs kept outside the model context.
    Artifacts {
        #[command(subcommand)]
        command: ArtifactsCmd,
    },
    /// Remembered approval decisions: list, add, revoke.
    Rules {
        #[command(subcommand)]
        command: RulesCmd,
    },
    /// MCP servers Aether connects to, and Aether's own MCP server.
    Mcp {
        #[command(subcommand)]
        command: McpCmd,
    },
    /// Inspect, toggle, or cancel global voice replies in an already-running TUI.
    /// Meant to be bound to a desktop global hotkey; does nothing on its own
    /// and starts no daemon or TUI.
    Voice {
        #[command(subcommand)]
        command: VoiceCmd,
    },
    /// Cancel an agent and its children.
    Cancel {
        agent: String,
        #[arg(long)]
        json: bool,
    },
    /// Stop an agent's running turn; it keeps its transcript and waits for
    /// the next message.
    Interrupt {
        agent: String,
        #[arg(long)]
        json: bool,
    },
    /// Select repositories and configure the opt-in GitHub watcher.
    GithubWatcher {
        #[command(subcommand)]
        command: GithubWatcherCmd,
    },
    /// Manage durable recurring triggers.
    Schedule {
        #[command(subcommand)]
        command: ScheduleCmd,
    },
    /// Deliver an external trigger (webhook, timer, CI result) to an agent.
    Trigger {
        agent: String,
        /// Where it comes from, e.g. `ci` or `cron`.
        source: String,
        /// JSON payload; plain text is wrapped as a string.
        #[arg(long, default_value = "null")]
        payload: String,
        #[arg(long)]
        json: bool,
    },
    /// Recreate a failed or cancelled agent from its last checkpoint, with
    /// its transcript tail, as a new agent linked to it.
    Restart {
        agent: String,
        #[arg(long)]
        json: bool,
    },
    /// Wake a failed agent in place: same agent, same history and context,
    /// reachable by message again.
    Wake {
        agent: String,
        #[arg(long)]
        json: bool,
    },
    /// Pause an agent: it takes no turns until resumed, and its inbox fills.
    Pause {
        agent: String,
        /// Pause every live agent beneath it as well.
        #[arg(long)]
        subtree: bool,
        #[arg(long)]
        json: bool,
    },
    /// Lift a pause, or clear a wait or an inspection the operator owns.
    Resume {
        agent: String,
        /// Lift the pause on every live agent beneath it as well.
        #[arg(long)]
        subtree: bool,
        #[arg(long)]
        json: bool,
    },
    /// Set any of the agent's knobs on a live agent; each becomes an event.
    Set {
        agent: String,
        #[command(flatten)]
        knobs: cli::SetKnobs,
        #[arg(long)]
        json: bool,
    },
    /// Compact an agent's context promptly; a reply runs only if other work is pending.
    Compact {
        agent: String,
        #[arg(long)]
        json: bool,
    },
    /// Browse and curate durable memory.
    Memory {
        #[command(subcommand)]
        command: MemoryCmd,
    },
    /// Control this daemon from a phone or browser, and reach other
    /// machines' daemons over SSH.
    Remote {
        #[command(subcommand)]
        command: RemoteCmd,
    },
    /// Copy bytes between standard streams and the daemon's socket. What
    /// `--remote` runs on the far side of SSH; it starts no daemon.
    Bridge {
        /// Required: the only carrier there is.
        #[arg(long, required = true)]
        stdio: bool,
    },
    /// MCP bridge spawned by engines (Claude Code, Codex); not for direct use.
    #[command(hide = true)]
    McpBridge {
        #[arg(long)]
        agent: String,
        #[arg(long)]
        token: String,
    },
}
use crate::cli;
