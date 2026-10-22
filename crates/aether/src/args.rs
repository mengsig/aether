use clap::{CommandFactory as _, Parser, Subcommand};
use std::path::PathBuf;

mod remote;
mod subcommands;
pub(crate) use remote::*;
pub(crate) use subcommands::*;

/// The display flags only the terminal UI reads.
const TUI_ONLY_FLAGS: [&str; 5] = ["theme", "reduced_motion", "plain", "ascii", "view_mode"];
/// Commands that never draw the UI, so their help leaves those flags out.
const HEADLESS_COMMANDS: [&str; 2] = ["daemon", "serve"];

/// The command line with the UI-only flags hidden from the help of the
/// daemon commands. They stay accepted, since the flags are global.
pub(crate) fn command() -> clap::Command {
    let mut command = Args::command();
    command.build();
    for name in HEADLESS_COMMANDS {
        // `mut_args` keeps each argument in place; removing and re-adding one
        // with `mut_arg` made the last flag lose its value.
        command = command.mut_subcommand(name, |headless| {
            headless.mut_args(|arg| {
                if TUI_ONLY_FLAGS.contains(&arg.get_id().as_str()) {
                    arg.hide(true)
                } else {
                    arg
                }
            })
        });
    }
    command
}

#[derive(Debug, Parser)]
#[command(
    name = "aether",
    version,
    about = "Provider-agnostic, event-driven agent harness",
    after_help = "Daemon start: `tui`, `run`, `new`, `models`, `providers`, `auth`, `github-watcher` and `remote setup` start the daemon when none is running; the other commands that talk to it, such as `status`, `agents`, `tasks`, `send` and `schedule`, fail with \"no daemon\" instead. `doctor`, `schema`, `diagnose` and `bridge` start none.\n\nFor commands with --json, command errors print one object on stdout: {\"error\":{\"code\":\"...\",\"message\":\"...\",\"socket\":\"...\"}}. A human error also goes to stderr; exit codes are unchanged. Argument parsing errors retain Clap's usage output."
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
        agent: Option<cli::AgentRef>,
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
        /// Gate approvals for this run: the opposite of `--yolo`.
        #[arg(long = "no-yolo", overrides_with = "yolo")]
        no_yolo: bool,
        /// A JSON file of slash commands the host program behind the agents
        /// runs (`{"version":1,"commands":[...]}`); the menu lists them and
        /// sends them to the viewed agent as typed.
        #[arg(long, value_name = "PATH")]
        host_commands: Option<PathBuf>,
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
    /// Print this binary's version and protocol version (no daemon needed).
    Version {
        /// Print the result as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Inspect storage usage, or collect eligible files.
    Storage {
        #[command(subcommand)]
        command: Option<StorageCmd>,
        /// Print the result as JSON.
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
    /// Show daemon status (fails if no daemon is running).
    Status {
        /// Print the whole status as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Stop the daemon. Agents resume when it starts again; a turn with uncertain
    /// tool effects waits for `aether resume AGENT_ID`.
    Stop {
        /// Print the result as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Create an agent with an objective and stream its work to stdout (starts
    /// the daemon if needed).
    #[command(
        after_help = "Exit codes: 0 = completed or waiting for operator input; 1 = failed run or command error; 2 = invalid command-line arguments; 3 = cancelled run. Usage and --json output are printed before the run's exit status is returned. Delegated runs also show subtree usage, including descendants."
    )]
    Run {
        /// Workspace on the selected host; required for --remote.
        #[arg(long)]
        workspace: Option<PathBuf>,
        /// The objective, as one argument or several words; `-` or nothing
        /// reads it from stdin.
        #[arg(conflicts_with = "file")]
        objective: Vec<String>,
        /// Read the objective from a file.
        #[arg(long, value_name = "PATH")]
        file: Option<PathBuf>,
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
        /// Most tokens the run may spend, cache reads and writes included. An
        /// engine such as Claude Code spends over 20k on its first turn, so
        /// a smaller budget fails the run at once.
        #[arg(long)]
        token_budget: Option<u64>,
        /// Most dollars the run may spend.
        #[arg(long)]
        cost_budget: Option<f64>,
        /// Most turns the run may take.
        #[arg(long)]
        max_turns: Option<u32>,
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
        /// Print the result as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Show credential status per provider (starts the daemon if needed).
    Auth {
        /// Print the result as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Check the installation: config, daemon, store, disk, providers,
    /// engines and MCP servers. Exit code 0 all well, 1 something to watch,
    /// 2 something broken.
    Doctor {
        /// Print the result as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Replace this binary with the latest released one. Run by hand, or
    /// offered by the check `aether` makes on launch (`[update] check`).
    #[command(
        override_usage = "aether update [OPTIONS]",
        after_help = "Exit codes: 0 = updated, or nothing to do; 1 = error, or a migration not confirmed, with the installed binary left as it was; 2 = invalid command-line arguments. With --check: 0 = up to date, 10 = a newer release exists. A network failure exits 1 with the error code `network_error` under --json. --json prints the outcome as one object and no progress lines; the exit codes are the same.\n\nBefore an upgrade the release's summary is printed with the \"Before updating\" notes of every release it crosses. When one of them migrates the store one way, the update asks before anything is downloaded; --yes skips the question, and with --json the notes and the question go to stderr.\n\nOnly a binary that install.sh installed in a directory you can write is replaced; for any other, the command that updates it is printed. A running daemon keeps the old build until `aether stop` and the next `aether`. Where a daemon that refuses a newer build cannot be signalled (macOS, and Linux before 6.5), a running daemon is not left behind: nothing is installed until `aether stop` has been run. Assets come from GitHub releases, or from AETHER_INSTALL_BASE_URL, and their SHA-256 is checked before anything is replaced."
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
        /// Install a release whose notes announce a one-way migration
        /// without asking first.
        #[arg(long, conflicts_with = "check")]
        yes: bool,
    },
    /// Write a private, redacted diagnostic directory (never starts a daemon).
    Diagnose {
        /// New directory to create; existing destinations are refused.
        /// Default: `aether-diagnose-<UTC time>` in the current directory.
        #[arg(long)]
        out: Option<PathBuf>,
        /// Collect this run instead of the most recent runs.
        #[arg(long, conflicts_with = "last")]
        run: Option<cli::RunRef>,
        /// Number of recent runs (1..=100).
        #[arg(long, default_value = "5", value_parser = clap::value_parser!(u32).range(1..=100))]
        last: u32,
        /// Print the bundle path and manifest as JSON.
        #[arg(long)]
        json: bool,
    },
    /// List models and whether their provider is configured (starts the daemon
    /// if needed).
    #[command(
        after_help = "Budgets: API providers accept token and turn limits, and dollar limits when a model price is configured (cost is estimated from list prices). Claude Code accepts token, turn and dollar limits (list-price estimates or provider-reported cost). Codex accepts token and turn limits; dollar limits are refused because it reports no enforceable dollar cost. Subscription API-equivalent estimates do not enforce dollar budgets."
    )]
    Models {
        /// Print the result as JSON.
        #[arg(long)]
        json: bool,
    },
    /// List providers, their auth and their live health (starts the daemon if
    /// needed).
    Providers {
        /// Print the result as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Runs: what they did, and moving them between daemons.
    Runs {
        #[command(subcommand)]
        command: RunsCmd,
    },
    /// List agents (fails if no daemon is running).
    Agents {
        /// Include finished and cleared agents, not just live ones.
        #[arg(long)]
        all: bool,
        /// Print the result as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Clear finished agents or runs out of the live view (they stay in the
    /// store, in reports and in replay), or bring them back with --undo.
    Archive {
        /// An agent or a run: an id, a unique id prefix, a live agent's name, or @last.
        target: Option<cli::TargetRef>,
        /// Every finished agent there is - the same as `/clear` in the TUI.
        #[arg(long)]
        all_finished: bool,
        /// Bring the target back into the view instead.
        #[arg(long)]
        undo: bool,
        /// Print the result as JSON.
        #[arg(long)]
        json: bool,
    },
    /// List tasks (fails if no daemon is running).
    Tasks {
        /// Print the result as JSON.
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
        owner: Option<cli::AgentRef>,
        /// Task this one waits for; repeat for several.
        #[arg(long = "depends-on")]
        depends_on: Vec<cli::TaskRef>,
        /// Print the result as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Create a taskless agent eligible to own a task. Uses daemon defaults
    /// for workspace, approval policy and model unless overridden here. Starts
    /// the daemon if needed.
    New {
        /// Name to refer to the agent by, in place of its id.
        #[arg(long)]
        name: Option<String>,
        /// Model as provider/model[,provider/model...] (default: daemon default).
        #[arg(long)]
        model: Option<String>,
        /// Print the result as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Step through an agent's turns as they happened: what it saw, what it
    /// did, what it cost.
    Replay {
        agent: cli::AgentRef,
        /// One frame per line, for another process.
        #[arg(long)]
        json: bool,
    },
    /// Print events, optionally following live.
    Events(crate::cli::EventArgs),
    /// Send a message to an agent.
    Send {
        /// The agent to message: an id, a unique id prefix, a live agent's name or @last.
        agent: cli::AgentRef,
        /// The message; `-` or nothing reads it from stdin.
        text: Vec<String>,
        /// Attach an image file (PNG, JPEG, GIF or WebP); repeat for more.
        #[arg(long = "image", value_name = "FILE")]
        images: Vec<PathBuf>,
        /// Deliver to every live agent beneath it as well, each its own item.
        #[arg(long)]
        subtree: bool,
        /// Print the result as JSON.
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
        approval: Option<cli::ApprovalRef>,
        #[command(flatten)]
        remember: RememberArgs,
    },
    /// Deny a pending tool call.
    Deny {
        #[arg(
            required_unless_present = "all_pending",
            conflicts_with = "all_pending"
        )]
        approval: Option<cli::ApprovalRef>,
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
        agent: cli::AgentRef,
        /// Print the result as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Stop an agent's running turn; it keeps its transcript and waits for
    /// the next message.
    Interrupt {
        agent: cli::AgentRef,
        /// Print the result as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Select repositories and configure the opt-in GitHub watcher. Every
    /// subcommand, `status` included, starts the daemon if needed.
    GithubWatcher {
        #[command(subcommand)]
        command: GithubWatcherCmd,
    },
    /// Manage durable recurring triggers.
    #[command(
        after_help = "A tick that finds its target paused, or busy under the default policy, is recorded as skipped in `schedule history`. It is dropped: it does not count towards --max-deliveries, and nothing is replayed after `aether resume`. `schedule list` shows how many occurrences were skipped."
    )]
    Schedule {
        #[command(subcommand)]
        command: ScheduleCmd,
    },
    /// Deliver an external trigger (webhook, timer, CI result) to an agent.
    Trigger {
        agent: cli::AgentRef,
        /// Where it comes from, e.g. `ci` or `cron`.
        source: String,
        /// JSON payload; quote plain text as a JSON string.
        #[arg(long, default_value = "null")]
        payload: String,
        /// Print the result as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Recreate a failed or cancelled agent from its last checkpoint, with
    /// its transcript tail, as a new agent linked to it.
    Restart {
        agent: cli::AgentRef,
        /// Print the result as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Wake a failed agent in place: same agent, same history and context,
    /// reachable by message again.
    Wake {
        agent: cli::AgentRef,
        /// Print the result as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Pause an agent: it takes no turns until resumed, and its inbox fills.
    Pause {
        agent: cli::AgentRef,
        /// Pause every live agent beneath it as well.
        #[arg(long)]
        subtree: bool,
        /// Print the result as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Lift a pause, or clear a wait or an inspection the operator owns.
    Resume {
        agent: cli::AgentRef,
        /// Lift the pause on every live agent beneath it as well.
        #[arg(long)]
        subtree: bool,
        /// Print the result as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Set any of the agent's knobs on a live agent; each becomes an event.
    Set {
        agent: cli::AgentRef,
        #[command(flatten)]
        knobs: cli::SetKnobs,
        /// Print the result as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Compact an agent's context now and wait for the outcome; a reply runs
    /// only if other work is pending. Exits 1 when the compaction failed.
    Compact {
        agent: cli::AgentRef,
        /// Print the result as JSON.
        #[arg(long)]
        json: bool,
        /// Return once the request is accepted instead of waiting for the outcome.
        #[arg(long)]
        no_wait: bool,
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
