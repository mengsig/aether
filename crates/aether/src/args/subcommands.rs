use crate::cli;
use clap::Subcommand;
use std::path::PathBuf;

#[derive(Debug, Subcommand)]
pub(crate) enum MemoryCmd {
    /// List durable memories, most important first.
    List {
        /// Only this scope: `global`, `role:<role>`, `agent:<id>`,
        /// `subtree:<agent id>`, `repo-role:<role>@<repository>`, or a
        /// repository id.
        #[arg(long)]
        scope: Option<String>,
        /// Full-text search over memory text and tags.
        #[arg(long)]
        text: Option<String>,
        /// Only memories carrying this tag; repeat for several.
        #[arg(long)]
        tag: Vec<String>,
        /// Only memories this kind of recorder authored. `operator`: filed by an
        /// operator of this daemon (local or remote), not imported, not a repo note.
        #[arg(long, value_parser = cli::parse_memory_recorder)]
        recorded_by: Option<aether_core::memory::MemoryRecorder>,
        /// Include superseded and forgotten memories.
        #[arg(long)]
        all: bool,
        #[arg(long, default_value_t = 40)]
        limit: usize,
        #[arg(long)]
        json: bool,
    },
    /// Show one memory in full, with its provenance and supersession chain.
    Show {
        id: String,
        #[arg(long)]
        json: bool,
    },
    /// Stop retrieving a memory. The record stays in history.
    Forget {
        id: String,
        #[arg(long)]
        reason: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Put a forgotten memory back. Undoes `memory forget`.
    Restore {
        id: String,
        #[arg(long)]
        json: bool,
    },
    /// Keep a memory in front of the model on every turn in its scope.
    Pin {
        id: String,
        /// Undo the pin.
        #[arg(long)]
        unpin: bool,
        #[arg(long)]
        json: bool,
    },
    /// Import a repository's committed notes (.aether/knowledge/*.md).
    Sync {
        /// Repository root (default: the daemon's workspace). Must be the
        /// daemon's workspace root or a live agent's, or inside one.
        #[arg(long)]
        root: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// File an old repository name under the id its checkout derives now,
    /// moving its memories there. Moved memories do not move back.
    Alias {
        /// The old repository id, e.g. `archon`.
        alias: String,
        /// The checkout whose derived id the alias resolves to.
        #[arg(
            long,
            required_unless_present = "repository",
            conflicts_with = "repository"
        )]
        root: Option<PathBuf>,
        /// The repository id the alias resolves to, named outright.
        #[arg(long)]
        repository: Option<String>,
        /// Report what would move; move nothing.
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        json: bool,
    },
    /// List repository aliases and the id each resolves to.
    Aliases {
        #[arg(long)]
        json: bool,
    },
}

/// Keep an answer as a durable rule. Without `--remember` nothing is kept.
///
/// The rule is the narrowest reading of the answer: the agent that was
/// answered, and this call's own command or path. Widening is explicit.
#[derive(Debug, clap::Args)]
pub(crate) struct RememberArgs {
    /// Remember this answer for later calls like it.
    #[arg(long)]
    pub(crate) remember: bool,
    #[arg(long)]
    pub(crate) json: bool,
    /// Cover every call of the tool, not just this command or path.
    #[arg(long)]
    pub(crate) tool_wide: bool,
    #[command(flatten)]
    pub(crate) scope: ScopeArgs,
}

/// Where a rule applies. A hand-written rule names its scope; answering an
/// approval without one keeps the answer for the agent that was answered.
#[derive(Debug, clap::Args)]
pub(crate) struct ScopeArgs {
    /// One agent only.
    #[arg(long, conflicts_with_all = ["subtree", "run", "everywhere"])]
    pub(crate) agent: Option<String>,
    /// An agent and everything it delegates to.
    #[arg(long, conflicts_with_all = ["agent", "run", "everywhere"])]
    pub(crate) subtree: Option<String>,
    /// Every agent in one run.
    #[arg(long, conflicts_with_all = ["agent", "subtree", "everywhere"])]
    pub(crate) run: Option<String>,
    /// Every agent this daemon will ever run. Say it deliberately.
    #[arg(long, conflicts_with_all = ["agent", "subtree", "run"])]
    pub(crate) everywhere: bool,
}

impl From<ScopeArgs> for cli::RuleScopeArgs {
    fn from(a: ScopeArgs) -> Self {
        Self {
            agent: a.agent,
            subtree: a.subtree,
            run: a.run,
            everywhere: a.everywhere,
        }
    }
}

#[derive(Debug, Subcommand)]
pub(crate) enum RunsCmd {
    /// List runs, newest first.
    List {
        #[arg(long, default_value_t = 20)]
        limit: usize,
        #[arg(long)]
        json: bool,
    },
    /// What a run did, composed from its events: agents, tasks, tools, cost
    /// per subtree, approvals, failures and the timeline.
    Report {
        run: String,
        /// The whole document as JSON, for another process.
        #[arg(long, conflicts_with = "md")]
        json: bool,
        /// The whole document as markdown, to keep beside the run. Without
        /// either flag a digest is printed.
        #[arg(long)]
        md: bool,
        /// Write to this file instead of stdout.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Write a finished run - events, snapshots, checkpoints, artifacts - to
    /// one file, to inspect it in another daemon.
    Export {
        run: String,
        #[arg(long)]
        json: bool,
        /// The file to write. The daemon opens it, so it must be reachable
        /// from the daemon's machine.
        #[arg(long)]
        out: PathBuf,
    },
    /// Recreate a run from such a file, under fresh ids, with provenance to
    /// where it came from.
    Import {
        file: PathBuf,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum ArtifactsCmd {
    /// List artifacts, newest first.
    List {
        #[arg(long)]
        agent: Option<String>,
        #[arg(long)]
        task: Option<String>,
        /// Skip artifacts whose bytes retention has dropped.
        #[arg(long)]
        with_bytes: bool,
        #[arg(long, default_value_t = 50)]
        limit: usize,
        #[arg(long)]
        json: bool,
    },
    /// Show one artifact and print its content.
    Show {
        id: String,
        /// Bytes of content to print.
        #[arg(long, default_value_t = 64 * 1024)]
        max_bytes: usize,
        /// Print the record as JSON instead of the content.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum McpCmd {
    /// Configured servers, their health and their tools.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Reconnect and rediscover tools, all servers or one.
    Refresh {
        /// Only this server.
        server: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Serve Aether's own surface over MCP so another process can drive it.
    Serve {
        /// Serve on stdin/stdout. The default, and the only mode without
        /// `--http`; naming it is allowed so a caller can be explicit.
        #[arg(long, conflicts_with = "http")]
        stdio: bool,
        /// Serve streamable HTTP on this address, e.g. 127.0.0.1:9200.
        #[arg(long)]
        http: Option<std::net::SocketAddr>,
        /// Environment variable holding the bearer token HTTP callers must
        /// present. Required with --http; Aether stores no token.
        #[arg(long, default_value = "AETHER_MCP_TOKEN")]
        token_env: String,
        /// Allow --http to bind outside loopback. This endpoint can create
        /// agents that run commands on this machine, and Aether terminates
        /// no TLS, so meaning it has to be said.
        #[arg(long, requires = "http")]
        allow_remote: bool,
    },
}

/// A global-hotkey command: no payload beyond the action, and no `--socket`
/// of its own - it reaches the TUI already attached to the resolved daemon
/// `--socket`/`AETHER_SOCKET`.
#[derive(Debug, Subcommand)]
pub(crate) enum VoiceCmd {
    /// Inspect the running TUI's reply eligibility and lifecycle; never toggles audio.
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Start recording a global voice reply, or finish and send the one in
    /// progress - the same toggle either way.
    Reply {
        #[arg(long)]
        json: bool,
    },
    /// Discard the global voice reply in progress, if any.
    Cancel {
        #[arg(long)]
        json: bool,
    },
}

// Parsed once, at startup, like [`Cmd`].
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Subcommand)]
pub(crate) enum GithubWatcherCmd {
    /// Show configuration, delivery counts and polling errors.
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Add or update a repository. A new repository defaults to automatic
    /// dispatch: a managed checkout and agent per issue/PR thread. Override with
    /// --agent (fixed target) or --spawn-role + --spawn-objective (thread template).
    /// Updating an existing repository without either flag leaves its
    /// dispatch mode unchanged.
    Add {
        repository: String,
        /// Deliver every request to this existing agent.
        #[arg(
            long,
            conflicts_with_all = [
                "spawn_role", "spawn_objective", "spawn_model", "spawn_effort",
                "spawn_token_budget", "spawn_cost_budget", "spawn_max_turns",
                "spawn_backend", "spawn_network",
            ]
        )]
        agent: Option<String>,
        /// Thread dispatch: role label for the agent spawned per thread.
        #[arg(long, requires = "spawn_objective")]
        spawn_role: Option<String>,
        /// Thread dispatch: task each spawned agent gets.
        #[arg(long, requires = "spawn_role")]
        spawn_objective: Option<String>,
        /// Thread dispatch: provider/model[,provider/model...] for spawned
        /// agents. Without it the daemon's default is frozen in now.
        #[arg(long, requires = "spawn_role")]
        spawn_model: Option<String>,
        /// Thread dispatch: reasoning effort: low, medium, high, xhigh, max.
        #[arg(long, requires = "spawn_role")]
        spawn_effort: Option<String>,
        /// Thread dispatch: token budget for each spawned run.
        #[arg(long, requires = "spawn_role")]
        spawn_token_budget: Option<u64>,
        /// Thread dispatch: dollar budget for each spawned run.
        #[arg(long, requires = "spawn_role")]
        spawn_cost_budget: Option<f64>,
        /// Thread dispatch: turn limit for each spawned run.
        #[arg(long, requires = "spawn_role")]
        spawn_max_turns: Option<u32>,
        /// Thread dispatch: host (default) or container:<image>.
        #[arg(long, requires = "spawn_role")]
        spawn_backend: Option<String>,
        /// Thread dispatch: override the network default; --spawn-network=false denies.
        #[arg(
            long,
            requires = "spawn_role",
            num_args = 0..=1,
            default_missing_value = "true",
            require_equals = true
        )]
        spawn_network: Option<bool>,
        /// Only these GitHub logins (comma-separated) may dispatch; each still
        /// needs write access. Omitted keeps an existing repository's list.
        #[arg(long, value_delimiter = ',', value_name = "LOGIN,...")]
        allowed_users: Option<Vec<String>>,
        #[arg(long)]
        json: bool,
    },
    /// Narrow who may dispatch for a repository, or --clear to allow every
    /// collaborator with write access again. Never widens the write check.
    AllowedUsers {
        repository: String,
        /// GitHub logins that may dispatch; replaces the current list.
        #[arg(
            value_delimiter = ',',
            required_unless_present = "clear",
            conflicts_with = "clear"
        )]
        logins: Vec<String>,
        /// Remove the allowlist.
        #[arg(long)]
        clear: bool,
        #[arg(long)]
        json: bool,
    },
    /// Name the GitHub logins whose own requests carry the local operator's
    /// authority, or --clear so that none does. A listed account is as
    /// powerful as you are; it must still pass each repository's write check
    /// and allowlist.
    OperatorUsers {
        /// GitHub logins with operator authority; replaces the current list.
        #[arg(
            value_delimiter = ',',
            required_unless_present = "clear",
            conflicts_with = "clear"
        )]
        logins: Vec<String>,
        /// Give no GitHub login operator authority.
        #[arg(long)]
        clear: bool,
        #[arg(long)]
        json: bool,
    },
    /// Remove a repository from the watch list.
    Remove {
        repository: String,
        #[arg(long)]
        json: bool,
    },
    /// Start polling globally, or enable one repository.
    Enable {
        repository: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Stop polling globally, or disable one repository.
    Disable {
        repository: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Change matching expressions or the polling interval.
    Configure {
        #[arg(long)]
        mention_regex: Option<String>,
        #[arg(long)]
        label_regex: Option<String>,
        #[arg(long)]
        poll_interval_secs: Option<u64>,
        #[arg(long)]
        json: bool,
    },
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Subcommand)]
pub(crate) enum ScheduleCmd {
    /// Create a timer that triggers an existing agent (--target), or starts a
    /// fresh root run in this directory on each occurrence (--objective).
    #[command(group(clap::ArgGroup::new("recurrence").required(true).args(["every", "cron"])))]
    Create {
        name: String,
        /// Agent whose inbox receives each trigger; requires --payload.
        #[arg(
            long,
            required_unless_present = "objective",
            conflicts_with_all = [
                "objective", "role", "model", "effort", "token_budget",
                "cost_budget", "max_turns", "backend", "network",
            ]
        )]
        target: Option<String>,
        /// Start a new root run with this task on each occurrence.
        #[arg(long)]
        objective: Option<String>,
        /// Spawned runs: role label (default: assistant).
        #[arg(long, requires = "objective")]
        role: Option<String>,
        /// Spawned runs: provider/model[,provider/model...]. Without it the
        /// daemon's default is frozen into the schedule now.
        #[arg(long, requires = "objective")]
        model: Option<String>,
        /// Spawned runs: reasoning effort: low, medium, high, xhigh, max.
        #[arg(long, requires = "objective")]
        effort: Option<String>,
        /// Spawned runs: token budget for each run.
        #[arg(long, requires = "objective")]
        token_budget: Option<u64>,
        /// Spawned runs: dollar budget for each run.
        #[arg(long, requires = "objective")]
        cost_budget: Option<f64>,
        /// Spawned runs: turn limit for each run.
        #[arg(long, requires = "objective")]
        max_turns: Option<u32>,
        /// Spawned runs: host (default) or container:<image>.
        #[arg(long, requires = "objective")]
        backend: Option<String>,
        /// Spawned runs: override the network default; --network=false denies.
        #[arg(
            long,
            requires = "objective",
            num_args = 0..=1,
            default_missing_value = "true",
            require_equals = true
        )]
        network: Option<bool>,
        /// Interval in seconds, preserving its cadence across restarts.
        #[arg(long)]
        every: Option<std::num::NonZeroU64>,
        /// Five-field cron expression: minute hour day month weekday.
        #[arg(long)]
        cron: Option<String>,
        /// IANA timezone for cron expressions (default: UTC).
        #[arg(long, requires = "cron", conflicts_with = "every")]
        timezone: Option<String>,
        /// Skip overdue windows instead of delivering one coalesced trigger.
        #[arg(long)]
        skip_missed: bool,
        /// While the target is busy, deliver once its previous trigger was
        /// consumed instead of skipping the occurrence.
        #[arg(long, conflicts_with = "objective")]
        queue_one: bool,
        /// JSON payload; plain text becomes a string. Required with --target.
        /// Spawned runs see it at
        /// the end of their task, so keep secrets out of it.
        #[arg(long, required_unless_present = "objective")]
        payload: Option<String>,
        /// Stop after this many deliveries or runs. Agent budgets also apply.
        #[arg(long)]
        max_deliveries: Option<std::num::NonZeroU64>,
        #[arg(long)]
        json: bool,
    },
    List {
        #[arg(long)]
        target: Option<String>,
        #[arg(long)]
        json: bool,
    },
    Inspect {
        id: String,
        #[arg(long)]
        json: bool,
    },
    Pause {
        id: String,
        #[arg(long)]
        json: bool,
    },
    Resume {
        id: String,
        #[arg(long)]
        json: bool,
    },
    Cancel {
        id: String,
        #[arg(long)]
        json: bool,
    },
    History {
        id: String,
        #[arg(long, default_value_t = 100)]
        limit: usize,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum RulesCmd {
    /// List remembered decisions.
    List {
        /// Include revoked rules.
        #[arg(long)]
        all: bool,
        #[arg(long)]
        json: bool,
    },
    /// Always allow calls matching this rule.
    Allow {
        #[command(flatten)]
        matcher: MatchArgs,
        #[command(flatten)]
        scope: ScopeArgs,
        #[arg(long)]
        note: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Always refuse calls matching this rule.
    Deny {
        #[command(flatten)]
        matcher: MatchArgs,
        #[command(flatten)]
        scope: ScopeArgs,
        #[arg(long)]
        note: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Stop a rule from deciding anything from here on.
    Revoke {
        id: String,
        #[arg(long)]
        reason: Option<String>,
        #[arg(long)]
        json: bool,
    },
}

/// What a rule matches. An absent field matches anything.
#[derive(Debug, clap::Args)]
pub(crate) struct MatchArgs {
    /// Tool name, e.g. `bash`.
    #[arg(long)]
    pub(crate) tool: Option<String>,
    /// Glob over the call's command, e.g. `git status*`.
    #[arg(long)]
    pub(crate) argument: Option<String>,
    /// Resource key prefix a touched resource must start with, e.g.
    /// `file:/repo/src`.
    #[arg(long)]
    pub(crate) resource: Option<String>,
}

/// Remote operations whose delivery the host could not confirm.
#[derive(Debug, Subcommand)]
pub(crate) enum OutboxCmd {
    /// List the operations that may or may not have run on the host.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Record that an operation did run, and stop tracking it. Sends nothing.
    Delivered {
        /// The operation's idempotency key, as `outbox list` prints it.
        key: String,
        #[arg(long)]
        json: bool,
    },
    /// Send an operation again under its original key, accepting that it may
    /// run a second time if the host no longer holds its receipt.
    Resend {
        key: String,
        #[arg(long)]
        json: bool,
    },
    /// Record that an operation is no longer wanted, and stop tracking it.
    /// Sends nothing; it may already have run.
    Discard {
        key: String,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum RetentionCmd {
    /// Preview eligible runs, protected reasons, payload bytes, and pending cleanup.
    Preview {
        #[arg(long)]
        json: bool,
    },
}
