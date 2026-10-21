use std::io::Write as _;
use std::path::{Path, PathBuf};

pub const CONFIG_TEMPLATE: &str = r#"# Aether configuration. Every setting is optional; `aether models` lists the
# models you can name here as provider/model.

[model]
# Model for agents that do not name one. Unset: the first ready provider's
# first model (after `aether login claude-code`, that is claude-code/default).
# default = "claude-code/opus"

# Compaction automatically uses a sibling model with the same credentials:
# Claude: Sonnet 5.5; Codex/OpenAI: GPT-6 Luna; other providers: agent model.
# Override either setting here. Engine checkpoints use a fresh isolated session.
# compactor = "codex/gpt-6-luna"
# compactor_effort = "medium"

# Optional authored per-family guidance for API models. All families default off.
# Native harnesses always provide their own guidance. Set `true`
# or `false` for every family at once, or name families:
# profiles = { claude = true, gpt = false }

# Optional Claude Code cache-write tier override. Unset preserves the native
# harness configuration. Supported overrides are "5m" and "1h".
# prompt_cache_ttl = "5m"

# Optional TUI token customization; /theme preferences stay in ui.toml.
# See docs/theme-overrides.md for colors, modifiers and all semantic tokens.
# [theme.tokens.accent]
# fg = "75"
# modifiers = ["bold"]

[models.tiers]
# Capability band per model, shown as `tier` by list_models and used to pick
# models without naming them: "fast", "balanced", "strong" or "frontier". Built-in
# models ship with one; set it here to rank others or to disagree.
# "openrouter/openai/gpt-5" = "strong"

[models.compaction_tokens]
# Context size at which an agent compacts on its own, by "provider" or
# "provider/model" (the model wins). Defaults: 256000 for Claude, 272000 for
# Codex, else three quarters of the window; never above the model's window.
# "claude-code" = 256000
# "codex/gpt-6.1-sol" = 200000

[approval]
# What a new agent gets when it inherits nothing: "yolo" (the shipped
# default) auto-approves every tool call, "ask" gates external side effects -
# network, web tools, MCP servers. That is the only class approvals gate.
#
# WARNING: shell commands and file edits inside the agent's scope are never
# gated by approvals, under either default. Nothing in the prompt holds them
# back - the agent's scope and its execution backend are the boundary, so
# run untrusted work with `--backend container:<image>`. Set "ask" to be
# prompted before the agent reaches the network or an MCP server, or toggle
# one agent with /yolo, --yolo and --no-yolo.
# default = "yolo"

# true skips every approval prompt for every agent and refuses to turn any
# back on (the same as --dangerously-skip-permissions); engines also run
# unsandboxed. Stronger than `default`, and not per-agent.
# yolo = false

[network]
# New agents may use native web tools and network-enabled commands by default.
# Set "denied" to opt out; children inherit their parent's scope.
# Existing agents and explicitly requested scopes are not changed.
# default = "allowed"

[providers]
# Cached usage becomes stale after this many seconds; reads never probe providers.
# usage_stale_after = 900
# Minimum seconds between explicit refresh attempts; must be positive.
# usage_refresh_interval = 600
# Probe deadline in seconds; must be positive.
# usage_refresh_timeout = 15

[scheduler]
# Maximum simultaneous model turns across the daemon; must be positive.
# max_concurrent_turns = 32
# Optional check-in even if no task or message arrived. Unset waits for events
# without spending tokens on timed check-ins. Approval waits are never bounded.
# wait_timeout_minutes = 30
# How long an engine-backed agent (Claude Code, Codex) may keep running after
# a harness tool has already ended its turn, before its process is stopped.
# engine_park_grace_secs = 20
# How long an engine-backed agent keeps waiting out network outages, timeouts,
# provider capacity and server errors (backing off up to 5 minutes between
# attempts and resuming its session) before it fails. 0 fails at once.
# engine_transient_retry_minutes = 120
# Maximum simultaneous turns on an engine (Claude Code, Codex) across the
# daemon; unset means only the limits above apply. Engine turns are what a
# subscription window is spent on, so this bounds burn rate directly.
# max_concurrent_engine_turns = 4
# Dollars a single run may spend before new delegations are refused. Turns
# already running are never cancelled, and a provider that reports no dollar
# cost counts as zero, so the figure is a floor.
# run_budget_usd = 25.0
# New turns wait while the filesystem holding the data directory has less
# free space than this, in MiB; a full disk refuses the writes a turn makes.
# Turns already running carry on. Unset means the smaller of 5 GiB and 2% of
# the disk; 0 never waits.
# min_free_disk_mib = 5120

# Completed-run history is checked hourly. Eligible history is permanently
# removed after seven days by default; use 10 for a longer recovery window.
# Live runs and runs with durable references are protected. This does not
# limit backups, native harness sessions, workspaces, or total disk usage.
#
# [storage]
# retention_days = 14
# workspaces_days = 14       # 0 disables; clean eligible workspaces use grace below
# engine_sessions_days = 14  # recorded now; deletion deferred
# backups_days = 14
# logs_days = 14             # 0 disables age only, size bound stays
# artifacts_days = 30        # explicit override; otherwise [artifacts] applies
# event_history_days = 7     # explicit override; otherwise [retention] applies
# workspace_grace_seconds = 3600
# backups_keep = 3           # keep newest N OR within retention
# migration_backups_keep = 2 # but never more than the newest N
# gc_interval_seconds = 3600
# free_space_floor_bytes = 5368709120 # unset inherits scheduler; 0 disables
# Wholly Git-ignored directories with these names never keep a finished
# workspace: GC deletes them first. Per checkout: [storage.repository_build_output_dirs]
# build_output_dirs = ["target", "node_modules", ".venv", "__pycache__", "dist"]

# [retention]
# completed_run_days = 7  # positive days; 0 is invalid

# Each agent's processes get their own private TMPDIR, TMP and TEMP under
# this root, one 0700 directory per agent. Unset means `t` beside the store.
# Use an absolute path on disk, not tmpfs, short enough (root plus 13 bytes,
# 50 at most) that tools can still create Unix sockets under it.
#
# [agents]
# temp_root = "/var/tmp/aether"

# Agents in a repository workspace build into a cache outside it, one
# partition per repository and run, so a run's isolated children share warm
# output and another run never writes into it. Unset root means
# `aether.build-cache` beside `aether.db`. A listed table replaces the default.
# Over max_bytes (default 15% of its filesystem), storage GC evicts partitions
# no live agent leases, least recently used first.
#
# [build_cache]
# root = "/var/cache/aether"
# max_bytes = 53687091200
# [[build_cache.env]]
# name = "CARGO_TARGET_DIR"
# dir = "cargo-target"

# How long artifact bytes are kept. Records are never removed, so an old run
# still says what it produced even once the payload is gone.
#
# [artifacts]
# Days of artifact bytes to keep; 0 keeps them forever.
# retain_days = 30
# Total artifact bytes to keep; 0 means no size limit. The oldest go first.
# retain_bytes = 2147483648

# Retrieval over durable memory. Scope, tags, recency and full-text search
# always run; an index adds a semantic pass on top of them.
#
# [memory]
# "none" (default), or "local" for a deterministic on-machine embedder that
# needs no network and no key.
# index = "local"

# The `web_fetch` tool is always available to agents whose scope allows
# network. `web_search` appears only once a provider is configured here.
# The credential is never stored by Aether: name the environment variable
# that holds it, and put it in a header - a key in the endpoint's query
# string would end up in an error message, and an error is a durable event.
#
# [web.search]
# endpoint = "https://api.search.brave.com/res/v1/web/search"
# results_pointer = "/web/results"
# snippet_field = "description"
# api_key_env = "BRAVE_SEARCH_API_KEY"
# api_key_header = "X-Subscription-Token"

# The protocol over TCP as well as the Unix socket, so a TUI or an
# orchestrator on another machine can attach - over an SSH tunnel, because
# Aether terminates no TLS. Everything the socket allows, this allows, behind
# one token: name the environment variable that holds it, and generate one of
# at least 32 characters (`openssl rand -hex 32`). A bind outside loopback
# also needs allow_remote, which means "I accept plaintext on the network".
#
# [listen]
# address = "127.0.0.1:7777"
# token_env = "AETHER_TOKEN"
# allow_remote = false
# max_connections = 64
# idle_timeout_secs = 300   # at least 3: the clients ping at a third of it

# Remote control: a phone or browser drives this daemon through a web app the
# daemon serves, over plain HTTP on loopback with `tailscale serve` (or another
# TLS reverse proxy on this machine) in front. Nothing listens until
# `aether remote enable`; devices are paired with `aether remote pair`. These
# are the fixed limits; what is enabled is changed with `aether remote`, live.
# See docs/remote.md.
#
# [remote]
# address = "127.0.0.1:8377"
# allow_remote = false      # bind outside loopback, in plaintext
# max_connections = 32
# idle_timeout_secs = 300
# push_previews = false     # put a short content preview in push notifications
# push_subject = ""         # VAPID contact (mailto: or https:); empty derives it
# push_hosts = []           # extra push services, e.g. ["push.example.org"]

# `aether serve` connects the outside world to inboxes: a signed POST becomes
# an external trigger on an agent, and an event subscription posts to a URL
# with bounded retries and a dead letter for what never landed. Secrets are
# named, never stored: a delivery carries `X-Aether-Timestamp`,
# `X-Aether-Nonce` and `X-Aether-Signature: sha256=<hex>`, the HMAC-SHA256 of
# "<timestamp>.<nonce>.<body>", and one outside five minutes or with a nonce
# already seen is refused, including after restart (webhook-replay.db).
# Webhook secrets need at least 32 bytes. A subscription must name a secret;
# unsigned delivery tells the receiver nothing. It must be https unless the receiver
# is on this machine, since Aether terminates no TLS and an event body carries
# model content. Redirects are never followed.
#
# [serve]
# address = "127.0.0.1:8787"
#
# [[serve.webhook]]
# path = "/hooks/ci"
# agent = "builder"          # an agent id, or the name of exactly one live agent
# source = "ci"
# secret_env = "AETHER_HOOK_CI_SECRET" # must differ from other routes' and subscriptions' secrets
#
# [[serve.subscription]]
# url = "https://example.com/aether"
# events = ["agent.failed"]  # empty posts every event
# secret_env = "AETHER_HOOK_OUT_SECRET"   # required
# max_attempts = 5
# dead_letter = "serve-dead-letter.ndjson"

# MCP servers. Their tools appear to agents as `mcp__<server>__<tool>` and go
# through the same approvals, idempotency keys and events as built-in tools.
# Give each server either a `command` (stdio) or a `url` (streamable HTTP).
#
# A tool you do not classify counts as `external` - an outside service Aether
# knows nothing about - so the default approval policy asks you before it
# runs. Declare the ones you trust: read_only, idempotent_mutation, mutation,
# external.
#
# `send_caller_meta = true` makes each call name its agent in `params._meta`
# (`aether/agent_id`, `aether/task_id`), set by Aether and never by the model,
# so a server can tell agents apart without a token in the conversation. Off
# by default; see docs/mcp-servers.md for the trust model.
#
# [[mcp.servers]]
# name = "files"
# command = "npx"
# args = ["-y", "@modelcontextprotocol/server-filesystem", "/srv/data"]
# classes = { read_file = "read_only", list_directory = "read_only" }
#
# [[mcp.servers]]
# name = "issues"
# url = "http://127.0.0.1:9100/mcp"
# token_env = "ISSUES_MCP_TOKEN"
# auth = "optional"   # connect without a token when the variable is unset
"#;

const PROVIDERS_HEADER: &str = r#"# Provider overrides and additions. Built-in providers (claude-code, codex,
# anthropic, openai, openai-codex, openrouter, groq, xai, google, ollama, vllm,
# lmstudio, llamacpp) need nothing here; `aether models` shows them.
# Model defaults belong in config.toml, not in this file.
#
# Disable a built-in provider:
#
# [providers.groq]
# enabled = false
#
# Opt into quota refresh turns through a native engine; may spend tokens.
# [providers.codex]
# usage_probe = "turn" # default "disabled"; config.toml controls probe limits
#
# Add an OpenAI-compatible server with its own models:
#
"#;

/// Valid on its own; written commented out under [`PROVIDERS_HEADER`].
pub const PROVIDERS_EXAMPLE: &str = r#"[providers.local-llm]
name = "Local llama-server"
protocol = "openai-chat-completions"
base_url = "http://localhost:8080/v1"
auth = { none = true }

[[providers.local-llm.add_models]]
id = "qwen3-coder"
name = "Qwen3 Coder (local)"
# A price is what tells a subscription agent's fallback guard this model is
# free; omit it and it is refused as unknown, metered cost.
capabilities = { context_window = 65536, max_output_tokens = 8192, cost = { input_per_mtok = 0.0, output_per_mtok = 0.0 } }
"#;

/// The only way a Claude Code model gets a 1M window: nothing ships it built in.
/// Valid on its own; written commented out under [`PROVIDERS_CLAUDE_1M_HEADER`].
pub const PROVIDERS_CLAUDE_1M_EXAMPLE: &str = r#"[[providers.claude-code.add_models]]
id = "claude-opus-5-5[1m]"
name = "Claude Opus 5.5 (1M context)"
capabilities_from = "claude-opus-5-5"
capabilities = { context_window = 1000000 }
"#;

const PROVIDERS_CLAUDE_1M_HEADER: &str = r#"#
# Claude Opus 5.5 with a 1M context window, as claude-code/claude-opus-5-5[1m].
# Never on by default; see "Claude context windows" in docs/engines.md.
#
"#;

pub fn providers_template() -> String {
    let mut out = String::from(PROVIDERS_HEADER);
    push_commented_out(&mut out, PROVIDERS_EXAMPLE);
    out.push_str(PROVIDERS_CLAUDE_1M_HEADER);
    push_commented_out(&mut out, PROVIDERS_CLAUDE_1M_EXAMPLE);
    out
}

fn push_commented_out(out: &mut String, example: &str) {
    for line in example.lines() {
        if line.is_empty() {
            out.push_str("#\n");
        } else {
            out.push_str("# ");
            out.push_str(line);
            out.push('\n');
        }
    }
}

/// Write `config.toml` and `providers.toml` templates where they do not
/// exist yet. Existing files are never touched. Returns the paths written.
pub fn ensure_templates(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    super::create_config_dir(dir)?;
    let mut written = Vec::new();
    for (name, text) in [
        ("config.toml", CONFIG_TEMPLATE.to_owned()),
        ("providers.toml", providers_template()),
    ] {
        let path = dir.join(name);
        // create_new: never overwrite, even if another process raced us.
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut file) => {
                file.write_all(text.as_bytes())?;
                written.push(path);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    }
    Ok(written)
}
