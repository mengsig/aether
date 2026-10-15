# Configuration

On first start Aether creates commented templates in the platform configuration
directory (`~/.config/aether/` on Linux unless `XDG_CONFIG_HOME` changes it;
`~/Library/Application Support/aether/` on macOS). `AETHER_CONFIG_DIR`
overrides that location. The [shipped templates](../crates/aether-model/src/config/templates.rs)
are the source for supported keys and defaults.

```toml
# config.toml — example choices, not a list of shipped defaults
[model]
default = "claude-code/opus"

[models.tiers]
"openrouter/openai/gpt-5" = "strong"

[scheduler]
max_concurrent_turns = 8
max_concurrent_engine_turns = 4
run_budget_usd = 25.0

[retention]
completed_run_days = 10
```

- **Model tiers:** every model carries a capability band within its provider,
  reported as `tier` by `list_models`: `fast` (Haiku, Luna), `balanced`
  (Sonnet, Sol), `strong` (Opus, Astra) or `frontier` (Fable; Codex has none
  today), or null when it cannot be ranked. A provider may lack a tier. The
  tier comes from, in order: `[models.tiers]` in `config.toml`
  (`"provider/model" = "fast" | "balanced" | "strong" | "frontier"`, aliases
  accepted; the legacy `strongest` reads as `strong`; an unknown tier is a load
  error), a `tier` on the catalog entry (also settable per model in
  `providers.toml`), then, for a priced model without one, the tier of the
  provider's tiered model nearest in output price ratio (a tie takes the lower
  tier). Subscription models use their API equivalent's price. A priced model
  with no tiered reference in its provider stays null, as do unpriced models
  such as `default`. The daemon catalog and generated clients expose the same
  tier as the agent tool.
- **Models and effort:** choose them per agent; switch with `/model` or
  `aether set`. A running turn finishes on its original model; the new choice
  applies when the agent starts its next turn. Fallback chains are explicit
  and validated against billing and execution constraints.
- **Concurrency and budgets:** control global/provider/subtree scheduling and
  per-agent limits. The run-dollar limit above blocks new delegation; it does
  not retroactively cancel running work, and unknown cost is not a hard cap.
- **Providers:** configure endpoints, credentials by reference, and provider
  concurrency in `providers.toml`.
- **Interface:** theme, motion, mouse, session defaults, pinned workspaces,
  and workspace discovery roots are stored separately in `ui.toml`; see the
  [TUI guide](tui.md) and
  [theme overrides](theme-overrides.md).
- **History and storage:** configure completed-run and artifact retention.
  Inspect eligible runs and protected reasons with `aether retention preview --json`.
  [Retention](log-retention.md) is not a total-disk quota or a backup.
- **Tools and integrations:** configure MCP servers, web search, webhook
  delivery, and optional voice. Native agents keep their native tool setup.

Daemon configuration changes generally require a restart. GitHub watcher
settings are stored in the daemon database and can be changed live.

## Common settings

These settings live in `config.toml`; dotted names identify the TOML
section and key. Omitted settings use the shipped defaults.

| Setting | Default | Purpose |
| --- | --- | --- |
| `model.default` | First model of the first ready provider | Choose the default `provider/model`. |
| `scheduler.max_concurrent_turns` | `32` | Bound simultaneous model turns. |
| `scheduler.max_concurrent_engine_turns` | Unset | Add a limit for native CLI turns. |
| `scheduler.run_budget_usd` | Unset | Refuse new delegation once recorded run spend reaches the limit. |
| `approval.default` | `"yolo"` | `"ask"` gates external effects; local mutations need an explicit agent policy. |
| `retention.completed_run_days` | `7` | Retain eligible completed runs this many days. |

`network.default` is `"allowed"`. Scope and native-engine restrictions still
apply; approval policy alone is not a sandbox. See the
[engine integration guide](engines.md) and [known limitations](known-limitations.md).

## Providers

One signed-in native CLI **or** one configured API provider is enough.
These are all the provider IDs in the [built-in registry](../crates/aether-model/src/builtin.rs).
Native engines keep their CLI's tools and subscription sign-in; hosted APIs
use separate API access. Grok and Gemini consumer subscriptions are not
supported sign-in routes: use an xAI or Google API key.
The native CLI login commands below are only needed if you have not already signed in to that CLI.

| Provider id | Kind | Sign-in or key variable | Checked for 1.0 |
| --- | --- | --- | --- |
| `claude-code` | Native Claude Code CLI | `aether login claude-code` (Claude subscription) | Unit and scripted-CLI tests; live paired runs on 2026-10-10/11 ([measurements](benchmarks.md)) |
| `codex` | Native Codex CLI | `aether login codex` (ChatGPT subscription) | Unit and scripted-CLI tests; live paired runs on 2026-10-10/11 ([measurements](benchmarks.md)) |
| `anthropic` | Hosted Messages API | `ANTHROPIC_API_KEY`; also `ANTHROPIC_AUTH_TOKEN` or imported Anthropic profile | Messages adapter: fake-server tests |
| `openai` | Hosted Chat Completions API | `OPENAI_API_KEY` | Chat adapter: fake-server tests |
| `openai-responses` | Hosted Responses API | `OPENAI_API_KEY` | Responses adapter: fake-server tests |
| `openai-codex` | Operator-configured Codex Responses dialect; disabled by default | No built-in login or key variable; supply endpoint and auth, then enable in `providers.toml` | Codex dialect: fake-server tests; disabled-default unit test |
| `xai` | Hosted xAI (Grok), Chat Completions API | `XAI_API_KEY` | Shared Chat adapter: fake-server tests |
| `google` | Hosted Google (Gemini), OpenAI-compatible API | `GEMINI_API_KEY` or `GOOGLE_API_KEY` | Shared Chat adapter: fake-server tests |
| `groq` | Hosted Groq, Chat Completions API | `GROQ_API_KEY` | Shared Chat adapter: fake-server tests |
| `openrouter` | Hosted OpenRouter, Chat Completions API | `OPENROUTER_API_KEY` or `aether login openrouter` (browser sign-in issues an API key) | Shared Chat adapter: fake-server tests; no live browser check |
| `ollama` | Local Ollama server | None by default | Shared Chat adapter: fake-server tests |
| `lmstudio` | Local LM Studio server | None by default | Shared Chat adapter: fake-server tests |
| `vllm` | Local vLLM server | None by default | Shared Chat adapter: fake-server tests |
| `llamacpp` | Local llama.cpp server | None by default | Shared Chat adapter: fake-server tests |

The native CLI measurements cover the named trials, not every model or account.
The [API conformance suite](../crates/aether-model/tests/conformance.rs)
replays recorded response fixtures through a fake HTTP server, shared by
providers using the same protocol. It does not verify each vendor's endpoint,
account access, or an installed local server. The API checks listed above
are automated coverage; live API service and account sign-in checks are not
recorded for 1.0.

`openai-codex` ships without an endpoint or OAuth login. The built-in ChatGPT
subscription route is `codex`, the official CLI. Further native CLIs can be
added by implementing the [engine trait](engines.md#the-trait) and
[registering the engine](engines.md#registering-one); they are not enabled
by adding an HTTP provider.

Keys named above must be available in the daemon's environment. Alternatively,
`aether login xai`, for example, prompts for and stores an API key. A stored
credential takes precedence over environment keys. Anthropic also supports
importing an `ant` profile; this is distinct from Claude Code's CLI login.
Local providers need a running server with a model loaded; configure auth
explicitly if your server requires it.

`providers.toml` can also add a model to a built-in provider: Claude's 1M
context window is never on by default, and
[Use the 1M window](engines.md#use-the-1m-window) shows the entry that adds it.

### Add an OpenAI-compatible endpoint

Add this to `providers.toml` in the configuration directory. It matches the
[shipped example](../crates/aether-model/src/config/templates.rs) and assumes
a local server serving `qwen3-coder` at `http://localhost:8080/v1` without
authentication. Replace the URL, model ID and capability limits to match
your server. Aether appends `/chat/completions` to the base URL.

```toml
[providers.local-llm]
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
```

The zero prices describe a free local server; do not copy them for a paid
endpoint. For an endpoint requiring a key, replace the `auth` line with
`auth = { api_key_env = ["MY_PROVIDER_API_KEY"] }` and make that variable
available to the daemon. Restart the daemon after editing; the model is
then selectable as `local-llm/qwen3-coder`. This example's configuration is
checked locally; inference still depends on your server and model.

## State and connections

`AETHER_DATA_DIR` overrides the platform data directory, which holds
`aether.db`, logs, artifact payloads, and the images sent with messages
(`aether.attachments/`). `AETHER_SOCKET` selects a daemon
socket; `--socket` overrides it for a CLI invocation. The default is
`$XDG_RUNTIME_DIR/aether/aetherd.sock` where a runtime directory is available,
otherwise `aetherd.sock` in the data directory.

Optional `[listen]` settings expose the protocol over token-authenticated TCP.
There is no built-in TLS; use an SSH tunnel. `[serve]` settings configure the
separate webhook/event bridge. See the [protocol](protocol.md),
[MCP](mcp-servers.md), and [GitHub watcher](github-watcher.md) guides.

### Remote control

`[remote]` holds the fixed limits of [remote control](remote.md). Whether the
listener is enabled, its public origins and push are changed live with
`aether remote`, never here. Nothing listens until `aether remote enable`.

```toml
[remote]
address = "127.0.0.1:8377"
allow_remote = false
max_connections = 32
idle_timeout_secs = 300
push_previews = false
push_subject = ""
push_hosts = []
```

| Key | Default | Meaning |
| --- | --- | --- |
| `address` | `"127.0.0.1:8377"` | `host:port` the listener binds once enabled. |
| `allow_remote` | `false` | Required to bind outside loopback; the listener speaks plaintext HTTP. |
| `max_connections` | `32` | At least 1. HTTP connections served at once, and WebSockets open at once. |
| `idle_timeout_secs` | `300` | At least 3. A WebSocket silent for this long is closed. |
| `push_previews` | `false` | Let a push notification carry a short preview of what an agent wrote. |
| `push_subject` | `""` | VAPID contact, a `mailto:` or `https://` URL; empty derives it from the first public origin. |
| `push_hosts` | `[]` | Extra push service hosts a device may subscribe with: exact names or `*.suffix`. |

Unknown keys and values that cannot mean what they say are refused when the
configuration loads.

## Storage

Filesystem maintenance runs in the background at startup and hourly. Inspect
usage and each cleanup decision with `aether storage`, or use
`aether storage gc --dry-run` before `aether storage gc`. These commands connect
to an existing daemon. `--json` includes byte counts, scan-limit flags, and the
last real sweep in this daemon process. Cleanup is host-only; paired remote
clients cannot request deletion.

```toml
[storage]
retention_days = 14
# workspaces_days = 14
# engine_sessions_days = 14
# backups_days = 14
# logs_days = 14
# artifacts_days = 30
# event_history_days = 7
workspace_grace_seconds = 3600
backups_keep = 3
gc_interval_seconds = 3600
# free_space_floor_bytes = 5368709120
```

Per-kind day overrides inherit `retention_days`, except artifacts and event
history, which retain their existing `[artifacts]` and `[retention]` policies
unless explicitly overridden here. Zero disables cleanup of that kind; log
size bounds still apply. Day values must be integers from 0 through 65535;
`backups_keep` and `gc_interval_seconds` must be positive. Unknown keys and
invalid values fail configuration loading. A zero grace period is allowed.

Eligible clean delegated workspaces are removed after the grace period, without
waiting fourteen days. Ownership comes from an agent's workspace root at or
inside that workspace, the manifest delegator's ancestor/descendant chain,
or an explicit resource lease or integration. A broad ancestor scope or
`allowed_paths` permission alone does not establish ownership. A live,
Failed or Cancelled workspace owner, integration journal, dirty files, or
commits not saved on another source-repository ref protect the workspace
regardless of age. Read-only Git probes run before the runtime gate; GC
rechecks filesystem identity and admission under the runtime locks.
Ignored files and paths it cannot safely inspect may also protect a workspace;
retention never overrides the work-preservation checks. Inventory and cleanup
are bounded per pass, so large directories may take several sweeps and byte
counts marked partial are lower bounds.

Migration backups keep **both** the newest `backups_keep` snapshots and every
snapshot within the retention window. Unrecognized files are preserved. Logs
retain their existing seven 16 MiB segment limit and gain age rotation and
pruning under the same cross-process writer lock. Legacy logs remain untouched
because old versions may still write them. Age maintenance runs when a log
writer starts or writes; inactive streams remain bounded by size. See
[log retention](log-retention.md).

`free_space_floor_bytes` overrides the scheduler's low-disk floor. Unset keeps
the existing default (the smaller of 5 GiB and 2% of the filesystem), or the
configured `scheduler.min_free_disk_mib`. Zero disables the floor. New turns
wait below the floor, and optional log writes wait in a bounded memory buffer;
required recovery/database writes are still attempted.

New native engine sessions get durable ownership records independent of
completed-run history. `engine_sessions_days` inherits `retention_days` (14 by
default); zero disables deletion. Only exact recorded Claude Code or Codex log
files qualify. Their native session identity must match the record within the
first 64 KiB, and the file modification time, ownership creation time and latest
owner state must all be outside the retention window. Completed and Cancelled
owners qualify once their turns have ended. Failed owners remain resumable,
including when archived or blocked by limits, unless a durable restart successor
permanently prevents waking the original agent. Restart uses a fresh native
session, so it does not require the predecessor's native log. When history
expiry removes an agent, that same transaction records its terminal status and
latest state time in session ownership. This evidence permits later native-log
cleanup even with the default seven-day history and fourteen-day log windows:
an expired agent cannot be woken or restarted. A currently present agent row
always takes precedence over earlier expiry evidence.

Engine GC preserves user sessions, pre-upgrade sessions without ownership,
unknown paths, missing owner history without expiry evidence, shared or ambiguous
paths, references from
another agent, active turns, symlinks in any path component, hard links, files
owned by another user, and files writable by another user or group. Unsupported
or mismatched native headers are retained. No directory scan or filename alone
establishes ownership. Native headers identify a session, not the provenance of
every byte; see [filesystem limitations](known-limitations.md#files). Admission
is rechecked under the runtime gate and scheduler locks before unlinking; file
identity, size, permissions and modification/change times must still match the
inspection. Unexpected inspection or unlink errors fail the sweep visibly.

Engine reports include per-path actions and reasons, measured bytes, eligible
bytes and removed bytes. At most 10,000 recorded paths are inspected per sweep;
all ownership records participate in ambiguity checks. A daemon-local cursor
advances only on successful real sweeps, so retained or missing records do not
starve later paths. A dry run previews the next real sweep without advancing
that cursor. Byte totals are lower bounds when `truncated` is true. Ownership
records survive deletion; missing files are reported without rediscovery.

Protected engine logs and workspaces, legacy logs, and explicitly disabled
retention remain possible sources of growth; this policy is not a hard disk
quota. Logs whose owner history expired before final-state evidence was recorded
are retained. SQLite reuses expired history pages rather than necessarily
shrinking its file.

## Agent temp dirs

Each agent gets a private temp directory, exported as `TMPDIR`, `TMP` and
`TEMP` to the processes it starts: its engine CLI, and the commands and
background jobs it runs on the host.

```toml
[agents]
# temp_root = "/var/tmp/aether"
```

`temp_root` must be an absolute path. `~` and environment variables are not
expanded. When unset, the root is `t` in the data directory, beside
`aether.db`. Aether never picks `/tmp` as the root.

The daemon resolves the root to its real path once at startup, following any
symlinks in it, and uses only that path from then on: it is what agents get as
`TMPDIR` and what `aether doctor` and `aether storage` report. The daemon
creates the root with mode 0700. It refuses a root that belongs to another
user, that is writable by group or others, or that its owner cannot read, write
and search. It also refuses a root with a parent directory on its real path
that belongs to a user other than you or root, or that others can write without
the sticky bit. A `.aether-instance` file in the root records the instance that
owns it. A root claimed by another instance is refused, so give each daemon its
own `temp_root`. Before each agent process starts, the daemon checks the root
and its claim again. It checks the parent directories only at startup and when
it has to create the root again, so keep their ownership and permissions as
they are. At startup the daemon also removes staging files named
`.aether-instance.<id>` that an interrupted claim left in the root.

A configured `temp_root` that fails these checks stops the daemon from
starting. The default root does not: if it cannot be created or fails a check,
the daemon logs a warning, `aether doctor` reports it, and agents keep the
daemon's own temp directory, as they did before agent temp dirs existed.

Each agent's directory is named after the last 12 characters of its id (for
example `<root>/4K9Z2M7QX1AB`). It is created with mode 0700 just before the
agent's first process starts, and created again if it was deleted. If an
existing directory is a symlink, belongs to another user, or has any mode
other than 0700, the agent's turn or command fails until it is removed.

Startup and `aether doctor` warn when the root is on tmpfs or ramfs, and when
the root's real path plus 13 bytes exceeds 50 bytes, which leaves tools too little room
for Unix socket paths. Claude Code's sandbox sets `TMPDIR` to a `claude-<uid>`
directory inside the agent's, 12 bytes longer for a four-digit uid, so the
warning shows both lengths. On macOS the default root,
`~/Library/Application Support/aether/t`, plus an agent's directory is about
62 bytes, so this warning always appears there. Set `temp_root` to a short,
absolute directory that only you can write, such as `/Users/<you>/.aether-t`. `aether storage` reports the total bytes and the number
of agent directories under `agent_temp`. Nothing is cleaned up automatically
yet.
