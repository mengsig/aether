# Changelog

What each release of Aether brings, newest first. Aether follows
[Semantic Versioning](https://semver.org/spec/v2.0.0.html). The section of a
release is also the text of its GitHub release.

## 1.2.0

A hardening release. Secrets are masked in more places (native engine output,
MCP listings, any repeat of a known token), the webhook door keeps replay
protection across restarts, and the daemon survives its own crashes and
panics more cleanly. Hosts can show a command's result at once with a note
level, the daemon reads the fleet in linear time, and `make bench-release`
compares a release candidate with the last release before it ships.
Protocol version 24, store schema 25.

### New

- **`make bench-release`, a paired release benchmark.** It runs a frozen set
  of short coding tasks on the last release, the candidate and the plain
  native CLIs, for Claude Haiku 5.5 and GPT-6 Luna at low effort, in
  parallel under a load gate, a USD budget and a per-trial spend guard. A
  paired bootstrap over tasks gates the candidate: it fails when, at 95%
  one-sided confidence, its pass rate is more than 5 points below the old
  release's or its cost more than 15% above it. An exact McNemar test is
  reported beside it. Runs resume, and each trial gets its own daemon, data
  directory and short TMPDIR. It is a release-time check, never part of CI (see
  [Release bench](https://github.com/mengsig/aether/blob/main/docs/release-bench.md)).
- **Notes carry a level.** `append_note` takes `level`: `info` (the
  default), `applied` or `refused`, so a host can show an operator command's
  result at once instead of waiting for a model turn. The TUI labels the row
  `note`, `applied` or `refused`. The level is display-only: it wakes no
  agent and reaches the model only through `context: digest`.
- **`schedule list` counts skipped ticks.** Each line ends with the number of
  occurrences dropped while the target was paused, among the newest 1000
  recorded. A skipped occurrence does not count towards `max_deliveries` and
  is never replayed.
- **`aether run` shows the disk gate and engine retries** while it waits.
- **The Python client can speak to an older daemon.** `AetherClient` takes
  `protocol_version` (17 to 24, default 24) for the version its hello asks
  for, so a caller can reach a 1.1.x daemon, which serves 17 to 23.

### Security

- **Known secret values are masked verbatim.** The daemon seeds its redactor
  with the TCP listener's token and the values of every variable it treats as
  a secret (`[listen]`, `[serve]` and MCP `token_env`, provider API key and
  bearer variables), so tool output or an engine error that repeats one is
  masked even when it has no recognisable shape.
- **Native engine output is redacted.** Native tool results and engine stderr
  pass through the redactor before they are stored. The redactor also knows
  JWT, AWS secret key, Slack, GitLab, Stripe, npm and Hugging Face shapes,
  and private keys cut by stderr truncation.
- **`mcp list`, `mcp list --json` and `status --json` mask secrets** in an
  MCP server's URL password and secret-looking arguments.
- **MCP `token_env` is hidden from tools and engines,** and a stdio MCP
  server's `env` may not name a hidden variable.
- **The webhook door is hardened.** `aether serve` refuses a route whose
  secret is shorter than 32 bytes, answers malformed requests with 401, stale
  or replayed deliveries with 409 and bodies over 1 MiB with 413, and keeps
  replay records in an owner-only `webhook-replay.db` that survives restarts
  (see [Signed webhook door](https://github.com/mengsig/aether/blob/main/docs/configuration.md#signed-webhook-door)).
- **Special-purpose IPv6 destinations are refused** by the public address
  boundary, so an embedded IPv4 address cannot bypass it.
- **Engine tokens are 32 random bytes** compared in constant time.
- **The installer refuses URL userinfo and cleartext redirects.** wget now
  walks redirects itself, following only https hops, so release downloads
  through wget work again.
- **A socket path that is not a socket is refused** instead of deleted.

### Performance

- **`get_tree` is linear** in the fleet, and the heavy reads (`list_agents`,
  `list_runs`, `get_run_report`, `replay`, `get_tree`) run off the async
  workers. A subscription caches each agent's parent and run, so a streamed
  delta no longer reads the store.
- **Scope checks walk parents instead of decoding the fleet.**
  `send_message`, `cancel_agent`, `wait_for` and artifact ownership use a
  parent walk or a recursive query.
- **The scheduler reads only budgeted agents** and skips deadline reads when
  idle.
- **The TUI stops refetching the tree on lease renewals,** and its status
  view no longer decodes the whole fleet.

### Fixed

- **Completed runs expire again.** Lease renewals and memory admission events
  were stored without their run, so a run with a long turn was kept from
  history retention forever. Both now carry their run, and schema 25
  backfills the existing ones.
- **The scheduler survives a panic.** A panic or error in one maintenance
  step is logged and counted instead of ending the scheduler loop, the other
  steps still run, and no turn starts while a limit-enforcing step failed.
- **Artifact eviction commits with its event,** so a refused commit leaves the
  artifact live for the next sweep.
- **Orphaned engines are reaped after a daemon crash.** The daemon records
  each engine process group and kills the recorded groups at its next start
  while their pid and start time still match. The first stdin write error
  reaches the turn's outcome.
- **`aether serve` reconnects** to a restarted daemon and answers 503 while it
  is down, where every webhook used to fail with 404.
- **Turn-ending tools end the turn.** A side-effecting tool call that arrives
  after `complete_task`, `fail_task` or a wait is refused.
  `complete_task` and `fail_task` bound their payloads, require `structured`
  to be an object and treat `structured: null` as absent. `inspect_state`
  reports the cost budget, turn limit and deadline.
- **A deliberate park no longer marks a run's cost incomplete.**
- **Shell capture leaves the worktree.** Foreground shell logs go to the
  agent's temp directory, capped at 256 MiB, so they no longer keep an expired
  workspace from GC.
- **Workspaces:** integration writes new files 0644 or 0755 under the umask
  instead of 0600; a failed creation logs its cleanup errors and orphaned
  snapshot refs are swept; an isolated delegation from a subdirectory says
  why it needs the Git root; a discarded workspace is reported by name.
- **`read_artifact` replies stay under the frame limit,** and frames
  pipelined with `hello` decode.
- **Budgets read cleanly.** Budget reasons print without floating-point
  noise, and an exceeded cost budget warns before the stop with a readable
  message.
- **`aether mcp serve` follows JSON-RPC:** -32700 for unparsable input,
  -32600 for batches and non-objects, -32002 before the handshake.
- **CLI:** a closed stdout pipe exits quietly; error causes print once and
  hints name real commands; `send`, `run` and `trigger` refuse empty input
  and read stdin; agent names are validated at creation and the agents table
  aligns by display width; `--max-turns 0` clears the limit and a past
  deadline is refused; `doctor` validates `[model] default`; `logout`
  validates the provider and `login` explains a missing terminal;
  `aether providers` shows a keyless local server that nothing answers at as
  unreachable, as `doctor` does.
- **Daemon logs:** no ANSI escapes when not on a terminal, retention at DEBUG,
  and `events --follow` ends with its stream.
- **TUI:** `TERM=dumb` selects plain mode, and a PAUSED badge and the
  `/pause` and `/resume` echoes stay visible in narrow layouts.

### Docs

- Which commands start the daemon, every long option's help, the CI table,
  orphaned delegated children and unlimited delegation depth, skipped schedule
  ticks, and the release bench.

### Protocol

The protocol is now version 24; this build still serves clients from 17.

- **24:** `level` on `append_note`. A level other than `info` needs a
  version 24 daemon: the Rust client refuses it to an older one before
  sending. The level rides in the note event's payload, which an older
  client decodes without it and shows as a plain note.

### Before updating

- **Store schema 25.** Backfills the run of lease renewals and memory
  admission events. The migration is one way: Aether saves the old database
  first (see
  [migration backups](https://github.com/mengsig/aether/blob/main/docs/migration-backups.md)), and a binary that predates
  schema 25 refuses to open a migrated database. To go back, stop the daemon
  and restore that backup with the older binary; changes made since are lost.
- **Webhook secrets need 32 bytes.** `aether serve` refuses to start with a
  route whose `secret_env` value is shorter. Keep `webhook-replay.db` beside
  `aether.db` across restarts.
- **`aether trigger` requires a JSON payload,** and `complete_task` and
  `fail_task` refuse a `structured` that is not an object.

## 1.1.1

Delegated swarms can now coordinate directly: workers find their siblings with
`list_peers`, message them with a `send_message` that stays within the team,
and read the recipe from `swarm_playbook`. The swarm tools are withheld from
agents they cannot help, so a lone root agent's tool list barely grows.

### New

- **Swarms: delegated peers find and message each other.** A lead can split
  one change across parallel leaf workers in isolated workspaces, and the
  workers coordinate directly instead of relaying through the lead.
  `list_peers` pages a delegated agent's siblings (id, name, role, status,
  task and a short objective), and a sibling's `send_message` wakes a peer
  waiting on its inbox or on `wait_for(message_from)`. `swarm_playbook`
  returns the recipe: disjoint tasks with owned paths, `leaf: true` workers
  with `allowed_paths`, peer coordination, and integration in plan order.

### Changed

- **A delegated agent's messages stay within its team.** `send_message` from
  a delegated agent reaches only its parent's subtree and its own ancestors,
  and a `subtree=true` fan-out only its parent's subtree. A refused message
  is an error the model sees, naming the rule. Root agents keep their reach.
- **Swarm tools cost nothing where they cannot help.** `list_peers` is
  offered only to delegated agents, since a root agent has no peers, and
  `swarm_playbook` only to agents that may delegate. A lone root agent's
  tool list grows by 185 bytes over 1.1.0: 173 from the playbook's short
  descriptor and 12 from a `send_message` description tweak;
  the playbook's text is returned by the call, not carried in the
  description.

## 1.1.0

Delegated agents now share a warm, size-bounded build cache outside their
workspaces, and an operator can diff, integrate or discard a child's work,
answer pending approvals in batches, follow `aether events` as a filtered
timeline and name agents by name, prefix or `@last` in every command.
Provider quota headroom and reset times are visible, and stale usage can be
refreshed on request. `aether update` shows the release notes and warns
before a one-way migration, and a daemon that outlives an update keeps its
MCP tools. Protocol version 23.

### New

- **Delegated builds share an Aether-owned build cache.** Agents in a
  repository workspace build into a cache outside it, one partition per
  repository and run under `aether.build-cache` beside `aether.db`, so a
  run's isolated children share warm output instead of each building a cold
  `target/`. `[build_cache]` sets the root and the exported variables
  (default `CARGO_TARGET_DIR`); they are set for engine CLIs and host
  commands and are writable inside the engine sandboxes. Storage GC keeps
  the cache within `[build_cache] max_bytes` (default 15% of its
  filesystem), evicting partitions no live agent leases, least recently used
  first; the disk guard does the same when the disk runs low. Only
  partitions Aether marked as its own are evicted. `aether storage` lists
  each partition's bytes, last use and leases. A client can name its own
  directory with `config.build_cache` behind the `build_cache` feature.
- **Review a child's work from the CLI.** `aether workspace diff`,
  `integrate` and `discard`, and the protocol commands `workspace_diff`,
  `workspace_integrate` and `workspace_discard` behind the
  `workspace_review` feature, land or drop an isolated child's worktree
  without asking its parent. The diff carries files, diffstat, commits and a
  patch capped at 256 KiB. Integration runs the agent tool's preflight and
  conflict check and names every conflicting path without writing. Discard
  refuses while any live agent works in the worktree unless forced. Each
  turn records its workspace HEAD at start and end, so commits are
  attributed to turns.
- **Pending approvals in one list, answered in batches.** `aether approvals`
  lists pending approvals with their agent, tool, arguments, side-effect
  class and age, filtered by `--agent`, `--subtree` or `--run`.
  `aether approve` and `aether deny` take `--all-pending` with one of those
  scopes, confirm the count (`--yes` skips it) and answer only that snapshot.
- **`aether events` is a usable timeline.** On a terminal it shows the
  latest 50 matching events; `--tail N` bounds any mode and `-f` follows.
  Filter with `--agent`, `--run`, `--task`, `--type`, `--family`, `--since`
  and `--grep`; `--json` prints one event per line.
- **Names, prefixes and `@last` wherever an id is expected.** Every CLI
  argument that takes an agent, run, task, approval or schedule id accepts a
  unique id prefix, with or without its `agt_`-style part, an exact name
  unique among live agents or schedules, `@last` for the newest agent of the
  current run, and `@viewed` for the agent a running TUI shows. Ambiguity is
  refused with the newest candidates listed. Memory scopes and the
  `aether events` and `aether workspace` filters resolve the same way.
- **Provider quota headroom with reset times.** `aether providers`, the TUI
  usage view, the `providers` reply and a new read-only `provider_usage`
  MCP tool show each provider's remaining percentage and reset time per
  window, and whether the report is stale. `[providers] usage_stale_after`
  (default 900 seconds) sets when it is.
- **Stale provider usage can be refreshed on request.** The
  `refresh_provider_usage` protocol command and MCP tool, behind the
  `provider_usage_refresh` feature, renew stale observations without
  starting ordinary work. A probe spends a small native engine turn, so each
  provider must opt in with `usage_probe = "turn"` in `providers.toml`; the
  default is `"disabled"`. Attempts are spaced by
  `providers.usage_refresh_interval` (600 seconds) and bounded by
  `providers.usage_refresh_timeout` (15 seconds). A probe whose cleanup
  cannot be confirmed quarantines further refreshes of that provider until
  the daemon restarts.
- **Notes on an agent's transcript that wake nothing.** `append_note`
  (protocol 23, `notes` feature, operate access) records a note on an agent
  without queueing a message or starting a turn. With `context: digest` the
  agent's next turn sees it in a compact list. The TUI, `aether replay` and
  the phone's activity feed and conversation show notes.
- **A host can add its slash commands to the TUI menu.** `aether tui
  --host-commands <PATH>` reads a JSON file of commands, which appear in the
  `/` popup and `/help` after the built-ins, marked with the host's name
  (Archon's `/answer`, for example). Choosing one sends the line to the
  viewed agent; the TUI never runs it. A malformed, repeated or built-in
  named entry is left out, and an unreadable file loads none; the TUI starts
  and says why once.
- **`aether update` shows what a release asks of you.** Before downloading,
  it prints the target release's summary and the "Before updating" notes of
  every release it crosses, read from the `CHANGELOG.md` each release now
  publishes. When one announces a one-way store migration it asks for
  confirmation; `--yes` skips the question and no answer is a no.
  `aether status` and `aether doctor` show "running X, installed Y" for a
  local daemon, doctor warns while they differ, and `status --json` adds
  `installed_version`.
- **Migration backups are compressed and capped.** A schema migration
  writes its pre-migration snapshot as a verified zstd file
  (`.sqlite.zst`), and storage GC keeps at most
  `[storage] migration_backups_keep` (default 2) of them whatever their age.
  [Restoring a compressed backup](https://github.com/mengsig/aether/blob/main/docs/migration-backups.md)
  is documented.
- **Storage GC can prune ignored build output.** Declared build directories
  (`target/`, `node_modules/`, `.venv/`, `__pycache__/` and `dist/` by
  default, configurable per repository) that Git ignores no longer keep a
  finished workspace from being collected, and
  `aether storage gc --prune-build-output [--dry-run]` deletes that output
  before re-running every safety check.
- **Claude Code telemetry carries Aether ids.** When
  `CLAUDE_CODE_ENABLE_TELEMETRY` is set in the daemon's environment, Claude
  Code children get `aether.run_id`, `aether.agent_id` and `aether.task_id`
  appended to `OTEL_RESOURCE_ATTRIBUTES` and a `TRACEPARENT` derived from the
  run. Aether does not turn telemetry on itself.

### Fixed

- **A daemon that outlives `aether update` keeps its MCP tools.** The daemon
  launched whatever binary was installed as its MCP bridge, so after an
  update every new native session got a bridge that spoke a protocol the
  daemon did not, and lost its Aether tools. The daemon now publishes its
  own image into an owner-only `<data>/bin` at startup and bridges from it.
  `aether status` prints the bridge path and `aether storage` lists the
  copies under `bridge_copies`.
- **Clients negotiate a common protocol on every transport.** A local client
  asked only for its own protocol, so a newer client refused an older daemon
  that spoke an older version both knew; only SSH retried. Unix, TCP and SSH
  now negotiate down the same way, and the Rust client refuses a command the
  connection cannot carry before sending it. An MCP bridge that cannot reach
  a usable daemon now stays up and explains why in every tool call.
- **A missing MCP token fails auth instead of connecting anonymously.** A
  server whose `token_env` names an unset or empty variable is reported as
  `auth_failed` with the variable it expected and is never contacted.
  `mcp list`, the TUI and doctor show each server's auth state.
- **Doctor reports unreachable local providers.** No-auth local servers are
  probed with a short timeout and reported "not reachable at host:port"
  instead of ready, and doctor fails when the default model's provider is
  not ready. The TUI shows its `/login` hint only for the new `no_model`
  error and otherwise keeps the daemon's message.
- **Unknown slash commands reach the attached agent.** A `/name` that is not
  an Aether command is sent verbatim to the viewed live agent, so Archon's
  `/answer`, `/approve`, `/reject`, `/merge` and `/archon` work from the TUI
  and plain mode. The first one in a session says it was forwarded; with no
  live agent in view it is still refused.
- **Status forecasts spend from recent usage.** A run's rate covers the last
  15 minutes of usage instead of its lifetime, so an idle run forecasts
  nothing. Human output of `aether status` and `aether storage` prints tokens
  and bytes in readable units; `--json` keeps exact values.

### Protocol

The protocol is now version 23; this build still serves clients from 17.

- **21:** the `no_model` error code for creating an agent with no model named
  and no default. Older clients get `invalid_argument`.
- **22:** `workspace_diff`, `workspace_integrate` and `workspace_discard`,
  behind the `workspace_review` feature.
- **23:** `append_note`, behind the `notes` feature, and the `not_started`
  replay frame that holds notes of an agent with no turn. Older clients do
  not receive that frame.
- Additive in this release: `headroom` on each `providers` row,
  `config.build_cache` on `create_agent` behind the `build_cache` feature,
  `refresh_provider_usage` behind the `provider_usage_refresh` feature,
  `prune_build_output` on `storage_gc` behind the feature of that name, and
  `auth` on MCP server health.

### Before updating

- **Agents build into Aether's build cache.** Agents in a repository
  workspace now get `CARGO_TARGET_DIR` pointing at a partition under
  `aether.build-cache` beside `aether.db` instead of building into the
  worktree's `target/`. Storage GC evicts unleased partitions once they hold
  more than 15% of that filesystem. Set `[build_cache] root` or `max_bytes`
  to change this, or `env = []` to export nothing (see
  [Build cache](https://github.com/mengsig/aether/blob/main/docs/configuration.md#build-cache)).
- **At most two migration backups are kept.** Storage GC now removes
  migration backups beyond the newest `[storage] migration_backups_keep`
  (default 2) whatever their age, where it used to keep the newest three.
  Raise it before updating to keep more.
- **An MCP server with a missing token stops connecting.** A server whose
  `token_env` variable is unset or empty used to connect without a token; it
  now fails auth. Set the variable, or mark the server `auth = "optional"`
  if it is meant to work without one.

## 1.0.10

Each agent now gets its own private temp directory instead of sharing the
system's, and startup and `aether doctor` warn when that directory is on
tmpfs or its path is too long for sockets. Cost figures are more accurate: a
Claude Code request the API refuses costs a complete $0 instead of an
unknown amount, and a Claude Code receipt that can't be used leaves the cost
marked incomplete when the agent was seen spending.

### New

- **Each agent gets its own temp directory.** The processes an agent starts,
  meaning its Claude Code or Codex CLI and the host commands, background jobs
  and terminals it runs, get `TMPDIR`, `TMP` and `TEMP` pointing at a private
  0700 directory. That directory sits under `[agents] temp_root`, which
  defaults to `t` in the data directory. The root is resolved to its real
  path at startup. The daemon refuses to start on a configured root that is
  not private to the user or rwx for its owner, sits under a parent others can
  change, or that another instance has claimed. If the default root can't
  be used, it warns and agents keep the daemon's temp directory. Startup and
  `aether doctor` warn when the root is on tmpfs or too long for socket paths.
  The length warning also gives the length inside Claude Code's sandbox
  directory. `aether storage` reports their size under `agent_temp`. Nothing
  is deleted yet.

### Fixed

- **Unusable Claude Code receipts keep a cost floor when spend is observed.**
  Missing or invalid dollars, all-zero receipts, and missing or reset
  baselines leave an `Unreported` gap when the invocation streams usage or
  any receipt so far in the process reports nonzero per-turn usage.
  Model-less streams count too. Receipts without spend evidence keep their
  previous recovery behavior.
- **A Claude Code request the API refuses costs a complete $0.** When the
  API refused an agent's request with HTTP 400, 401, 403, 404 or 413, its
  all-zero receipt was treated as unknown cost and the agent showed an
  incomplete $0. The API does not bill those statuses, so such a receipt now
  counts as $0 with no gap. This holds only for the process's first receipt,
  with no usage streamed, no model output, every counter zero, and every
  earlier retry caused by one of those statuses. Otherwise the gap stays.
  That includes a retry after a dropped connection, a timeout, 429, 5xx or
  529. Any retry, new request or model output later in the same process
  withdraws the $0.

## 1.0.9

Cost figures are more accurate. A Claude Code agent that pauses and resumes
is no longer charged twice for the same tokens, and Codex spend is shown as
an estimate. Every model now reports a capability tier. The daemon no longer
churns the disk, writes far fewer commits while agents stream, and by default
it now cleans up finished workspaces, old backups and old logs. An
interrupted tool call no longer reads as a denial, and clipboard images paste
into the terminal with Ctrl+V.

### New

- **Every model reports its capability tier.** `list_models`, the daemon
  catalog and the generated clients now give each model a `tier`: `fast`,
  `balanced`, `strong` or `frontier`, or null when it can't be ranked. The
  tier is set by `[models.tiers]` in `config.toml`, then by the catalog
  entry, and otherwise by the price of the provider's nearest tiered model.
  The legacy name `strongest` is read as `strong`, and an unknown tier is a
  config error. The model guidance the agent tool returns no longer names
  specific models, so a new model release needs no prompt changes.
- **Storage maintenance has a single configurable policy and visible decisions.**
  `[storage]` defaults to fourteen-day filesystem retention, with per-kind
  overrides, a one-hour grace for safely saved final delegated workspaces,
  and the latest three migration backups protected. `aether storage` and
  `aether storage gc --dry-run` show usage and reasons; the daemon sweeps at
  startup and periodically. Dirty, live, resumable and journal-referenced
  workspaces are protected. Logs gain age rotation and pause optional writes
  below the configured disk floor. New engine sessions have durable ownership
  records. A Claude Code or Codex session log that Aether started is removed
  once its agent has completed or been cancelled and the retention window
  has passed. Your own sessions, sessions from before the update, unknown or
  shared paths, and sessions of failed agents, which can still resume, are
  kept.

### Fixed

- **An interrupted tool call no longer reads as a denial.** When the daemon
  stopped or restarted, an agent was interrupted or cancelled, or a stage
  ended, a Claude Code or Codex tool call could look as if the operator had
  denied it. The agent was never told otherwise. Transcripts now show it as
  interrupted, with the cause. On resume, the agent is told the call was
  interrupted rather than denied and may be retried. Genuine approval
  denials stay denied.
- **Clipboard images paste into the terminal UI with Ctrl+V.** Ctrl+V prefers
  offered plain text, otherwise attaches a supported image; Alt+V and `/image`
  force an image. Text keeps the usual long-text placeholders and image-path
  detection. A truly empty terminal paste also tries the clipboard image;
  whitespace-only pastes and plain-mode input stay text. Reads happen in the
  background; an empty clipboard, missing helper or SSH session gets a one-line
  explanation without changing the draft. Existing custom key bindings are
  preserved.
- **Codex usage now carries a dollar estimate that says it is one.** A Codex
  (ChatGPT subscription) turn used to be booked as billed dollars at the
  OpenAI list price, so a run with any unreported part, such as an interrupted
  one, left a total with no complete cost and no estimate; tools that enforce
  dollar budgets, like Archon, then stopped the work. Codex turns are now
  estimated from the API model behind the slug, the way a subscription model
  without a price already was, and show as `≈ $X API-equivalent` next to a
  cost of `not reported`. Dollar caps on Codex agents are still refused. Older
  records keep the figures they were stored with.
- **A Claude Code agent that pauses and resumes no longer costs more than its
  receipt.** Claude Code reports a turn's tokens for that turn alone, but its
  dollars and per-model token counts for the whole session. When the turn's
  tokens fell short of what had streamed, Aether priced the difference again
  although the session's counts already covered it, and the extra carried
  over every resume. Aether now prices only streamed tokens those counts do
  not cover, and the note on an interrupted turn says whether the missing
  part was priced. An all-zero crash receipt keeps the last baseline;
  lower nonzero receipts leave a cost gap and become the next turn's
  baseline. Such a turn still counts its tokens at list price as a lower
  bound beside that gap.
- **The daemon no longer churns the disk while agents work.** Reading a long
  agent history made SQLite sort it in throwaway files under `/var/tmp`, tens
  of MB per read. With several agents running, that held host IO pressure
  high enough to stall other programs' syncs. Those sorts now stay in memory,
  and what the store keeps, and when it syncs, is unchanged.
- **Agent lists and a supervisor's history no longer sort everything they
  read.** Listing agents or tasks sorted every stored record on each call:
  the terminal's status poll every 2 seconds, each tree refresh, and
  orchestrators listing agents all paid it. Reading an agent's events together
  with its children's sorted the events' contents. Those sorts held a copy of
  everything they read, in memory or on disk. Full listings, live agents, and
  one run's agents and tasks now read in creation order from an index, and a
  supervisor's history sorts only event numbers. In a test store, listing
  10,000 agents peaked at 2.1 MB instead of 40.8 MB, and reading a supervisor's
  85,715 events with its children's took 0.2 s and 4.1 MB instead of 1.1 s and
  412 MB.
- **One run's agent tree no longer reads every task in the store.** Listing a
  run's tasks walked the whole tasks table. In a test store of 20,000 tasks
  that took 22 ms per tree; it now reads only that run's tasks, in 0.06 ms.
- **Native engine turns write far fewer commits.** Claude Code and Codex
  report token usage and context size several times per message, and each
  report was its own commit. The daemon now keeps the latest figures
  in memory for live subscribers and budgets, and writes them with the next
  record the turn makes anyway, or after at most 2 seconds. Final usage is
  written exactly as before; a daemon crash can lose the last 2 seconds of
  streamed usage, and nothing is counted twice. Agents covered by their own
  or an ancestor's token or cost budget still persist each usage reading, so
  budget enforcement and warnings keep their existing timing.
- **Stopping a background job no longer gives up when one of its processes
  is exiting.** On Linux before 7.2, a process that is exiting briefly hides
  its environment, and the search for a job's escaped processes stopped there
  with an error. Escaped processes later in the search stayed alive, and a
  restarted daemon kept retrying the cleanup instead of finishing it. A
  process exiting mid-search is now skipped like one that has gone.
- **An SSH remote no longer mistakes a finished send for one in flight.** If
  the client started another program just as a send ended, the send's outbox
  entry stayed locked until that program had started. Resolving the outbox on
  connect could skip the entry, and `/outbox` could refuse it as being sent
  by another process. Each entry is now unlocked as soon as its send ends.

### Before updating

- **Codex spend is an estimate, not billed dollars.** A Codex turn now
  records `cost_usd` of 0 and the list-price figure as `estimated_cost_usd`.
  Limits that read only billed dollars, a parent's `cost_budget` over Codex
  children and `[scheduler] run_budget_usd`, no longer count Codex turns; a
  `cost_budget` warns once that its cost is a lower bound, `run_budget_usd`
  stays silent. Use token budgets for Codex work. An
  Archon with `count_estimated_cost` off sees $0 for Codex.
- **Storage cleanup is on by default.** Without a `[storage]` section, the
  daemon now cleans up at startup and every hour. A delegated workspace that
  is clean, whose agents are finished and whose commits are saved on another
  ref is removed one hour after it qualifies. Migration backups beyond the
  newest three and older than fourteen days are removed, and logs are pruned
  after fourteen days. Workspaces with live or resumable agents, dirty files
  or unsaved commits are always kept. Claude Code and Codex session logs
  that Aether starts after the update are removed fourteen days after their
  agent completed or was cancelled. Your own sessions, and sessions from
  before the update, are kept. To see what would go, run
  `aether storage gc --dry-run` after updating. To keep a kind, set its
  `*_days` to 0 (see
  [Storage](https://github.com/mengsig/aether/blob/main/docs/configuration.md#storage)).
- **Store schema 24.** Schema 23 adds indexes that list agents and tasks in
  creation order, both across the store and within one run. Its per-run
  agents index replaces the old run index. Schema 24 adds a table that
  records which agent owns each new engine session. The migration is one
  way. Aether saves the old database first (see
  [migration backups](https://github.com/mengsig/aether/blob/main/docs/migration-backups.md)),
  and a binary that predates schema 24 refuses to open a migrated database.
  To go back, stop the daemon and restore that backup with the older
  binary. Changes made since the update are lost.

## 1.0.8

Saying whether a memory helped or hurt a task now changes how it ranks in
recall, and you can see which memories each model call was given. The
terminal no longer shows older state after it reconnects to the daemon.

### New

- **You can record whether a memory helped or hurt a task.**
  `aether memory feedback record` keeps a verdict, where the evidence came
  from and a reason, and only for a memory the task's agents were given.
  Feedback can be retracted and listed, keyed retries never record it twice,
  and every change leaves an event. Agents and paired devices cannot use it;
  clients detect support with `memory_feedback`. Once a memory has helped or
  hurt verdicts from two tasks, they move its rank in recall: its score is
  scaled by 0.75 to 1.25, counting each task's latest operator or review
  verdict over the latest 20 tasks. Pinned memories are untouched, and
  feedback never hides a memory or changes how many are recalled.
  `aether memory feedback list` shows the current factor.
- **You can see which memories each model call was given.** The
  context record of every call now lists the memories it rendered, in the
  order the prompt shows them, for Claude Code and Codex turns too. The prompt
  itself is unchanged. Ask for a task or a single call with
  `aether memory context --task <id>` or `--invocation <id>`; clients can
  detect support with `memory_context`. This is the first step toward judging
  memories by how the work they informed turned out.

### Fixed

- **The terminal no longer goes back in time after a reconnect.** When the
  TUI or `--plain` reconnected to the same daemon, a late answer to a request
  sent before the outage could overwrite newer state: an agent that had
  failed showed as waiting again, or daemon-wide usage dropped to 0. Answers
  from the replaced connection are now dropped when the new connection reads
  the same thing again. Results of what you did still arrive, each settling
  only its own request, and after a switch to another daemon and back
  nothing from before the switch is applied.

### Before updating

- **Store schema 22.** Adds the table that keeps memory feedback. The
  migration is one way: Aether saves the old database first (see
  [migration backups](https://github.com/mengsig/aether/blob/main/docs/migration-backups.md)), and a binary that predates
  schema 22 refuses to open a migrated database. To go back, stop the daemon
  and restore that backup with the older binary; changes made since are lost.

## 1.0.7

Lessons can wait for your approval before agents recall them. A Claude Code
or Codex answer cut off partway now looks the same live and after a reload,
however it was stopped. Costs Aether works out itself now match Anthropic's
bill for cached prompts.

### New

- **Memories can wait for review before agents recall them.** Propose an
  operator lesson with `aether memory add --proposed`, then accept it when
  the evidence holds or reject it with a reason. Proposed replacements leave
  the old lesson in use until acceptance, which refuses text already admitted
  in the same scope. Restoring a rejected proposal reopens review without
  putting it into recall, and retrying acceptance or rejection does not write
  duplicate events. Clients can detect support with `memory_proposals`;
  `memory list --proposed` shows the review queue.

### Fixed

- **What a Claude Code or Codex turn streamed before it failed stays on the
  record.** The terminal showed the partial answer and reasoning live, but a
  reload showed nothing: the failure was stored under a new id with no
  content. The failure record now keeps the uncommitted reasoning and text
  under the id they streamed under, so both views show them once. Nothing is
  billed for it, and it is never sent back to the model.
- **The same holds when such a turn is cancelled or interrupted.** What had
  streamed is kept as a cancelled failure under the id it streamed under, so
  a reload matches the live view. It costs nothing, and a resumed turn does
  not see it as something the agent said.
- **It also holds when the turn's own budget check, the engine adapter, or
  an earlier decision ends it.** A turn that crossed its budget mid-answer,
  an engine its adapter stopped (for example Codex reaching for its own
  delegation tool), and text an engine kept streaming after the agent had
  already chosen to wait or finish now keep what streamed the same way. An
  adapter's stop still shows its reason and reads as an interruption, not a
  cancel or a provider error. Text sent after a wait or a finish is shown
  after it and never changes the wait or the task's result.
- **An API reply cut short by a budget or deadline no longer shows twice.**
  The stop drew the half-written reply, then the copy kept for the next turn
  drew it again. Words still arriving when it stopped are shown once, after
  it. A Claude Code or Codex message that completes the cut reply is still
  shown.
- **1-hour prompt-cache writes are priced at their own rate.** Claude Code
  writes 1-hour cache entries, which Anthropic bills at twice the input
  price, but Aether priced every cache write at the 5-minute rate (1.25x).
  Costs and API-equivalent estimates that Aether works out itself were too
  low: across one Haiku benchmark, pricing the tokens that way came to 16%
  less than Claude Code reported.
  Usage now carries `cache_write_1h_tokens`, the 1-hour part of
  `cache_write_tokens`, and model prices carry `cache_write_1h_per_mtok`.
  Claude Code's own reported cost was already right and is unchanged. Older
  records, and engines that do not report the split, are priced as before.
- **Claude Sonnet 5.5 cache reads cost half what Aether charged.** Anthropic
  bills them at $0.10 per million tokens (0.05x input), as for Opus 5.5, but
  Aether used $0.20. A turn reading 200,000 cached tokens is now priced at
  $0.02 for those reads instead of $0.04.

### Before updating

- **Store schema 21.** Adds the column that keeps proposed memories out of
  recall. The migration is one way: Aether saves the old database first (see
  [migration backups](https://github.com/mengsig/aether/blob/main/docs/migration-backups.md)), and a binary that predates
  schema 21 refuses to open a migrated database. To go back, stop the daemon
  and restore that backup with the older binary; changes made since are lost.

## 1.0.6

Operators can teach a role, list what they taught, and give one repository
a single memory however its agents name it. Incomplete costs now say why,
and several totals that wrongly read complete are now marked as floors.

### New

- **Operators can file memories by role.** The `role` scope reaches agents
  with that exact role across repositories. The `repository_role` scope
  reaches one role family in a repository, including dotted subroles such
  as `reviewer.security` and Archon roles such as
  `archon-stage:default/review`.
  Both repository scopes accept `repository` to target an existing agent's
  explicit repository name, or `root` for the workspace-derived id. Daemons
  advertise `operator_memory_roles` so clients can check support for the new
  scopes and the `repository` field.
- **Memories can be listed by who recorded them.** The `memories` query
  takes `recorded_by: "operator"` to list only what an operator of this
  daemon filed, locally or remotely, leaving out what agents remembered,
  repository notes and memories read out of an import.
  `aether memory list --recorded-by operator` does the same. Daemons
  advertise `memory_recorded_by`; an older daemon would ignore the field and
  list everything, so clients check for it first and the CLI refuses to ask
  one.
- **A client can name the checkout an agent's memory belongs to.**
  `create_agent` takes `repository_root`, an absolute path the daemon derives
  the repository id from, exactly as for an agent working there, so agents in
  worktrees share memory with sessions in the checkout itself. It cannot be
  combined with `scope.repository`, and children inherit the result.
  `aether run --repository-root` does the same. Daemons advertise
  `repository_root`; an older daemon would ignore the field, so clients check
  for it first and the Rust client refuses to send it to one.
- **An old repository name can be aliased to the id its checkout derives.**
  Memories filed under a bare name such as `archon` (what Archon up to 1.2.1
  sends) can now join the id direct sessions and `repository_root` use, such
  as `github.com/mengsig/archon`. `aether memory alias archon --root PATH`
  (or `--repository ID`) moves every memory filed under the old name, and
  agents still naming it read and write the new scope from then on;
  `--dry-run` reports what would move first, and `aether memory aliases`
  lists them. The move is one transaction and cannot be undone. Daemons
  advertise `repository_aliases`, and paired devices cannot register one.

### Fixed

- **Incomplete costs now say why.** A usage total that is a lower bound
  (`cost_complete: false`) now carries `cost_gap`: `unpriced` (tokens with
  no known price), `unreported` (the engine did not report all its usage),
  `interrupted` (work stopped after the prompt was sent, before its closing
  accounting) or `mixed`. Records stored earlier read as `unknown`. Complete
  totals omit the field, and older clients ignore it. Cancelling a submitted
  engine turn now leaves its cost a lower bound even when the engine sends a
  final receipt. Harness stops of in-flight requests also stay interrupted;
  a receipt received before the stop stays complete if no new request
  followed it. Terminal parks still allow final accounting before their
  deadline. An API stream that fails midway keeps what it saw as a
  floor instead of reading complete. Failures before launch still add no
  spend. Cost text names the reason, e.g. "≥ $0.42 (interrupted run, cost
  unknown)".
- **A parent's terminal history is no longer crowded out by its children's
  tool activity.** Opening an agent fetched its newest 5,000 events together
  with its children's, though a child's tool calls draw nothing in the parent.
  A parent with 300 turns whose child then made 2,600 tool calls opened on
  none of its turns and had to page up to reach them. History pages now take
  only the child events the parent's transcript shows, and the same parent
  opens on all 300. An older daemon ignores the narrower request and pages as
  before.
- **A failed answer's reasoning survives a reload.** When an API response
  failed partway, the terminal showed its partial reasoning live but lost it
  after a reload, and lost it in both views when the answer had begun too.
  The stored reasoning now shows beside the partial answer in both.

## 1.0.5

Claude Code agents can opt into a leaner start that skips the operator's
skill listing and extra MCP servers, and engine costs now include requests
that a closing receipt left out.

### New

- **Lean host context for Claude Code agents.** An agent created with
  `host_context: "lean"` starts Claude Code without the operator's skill
  listing or MCP servers other than Aether's own. CLAUDE.md, hooks, settings,
  plugins and built-in tools stay. Delegated children inherit it unless
  `delegate` sets `host_context`. It is off by default; measured with the
  operator's setup on Haiku, the first prompt was 38% smaller.

### Fixed

- **Engine costs include requests missing from the closing receipt.** Streamed
  tokens omitted when Claude Code stops after a wait are now priced beside the
  reported cost. When no billed rate is available, the cost is marked incomplete
  and an API-equivalent estimate is shown where available.

## 1.0.4

Keep live replies visible while children work and when navigating approvals.
Recalled memories show their origin and precedence, changed requirements
are kept, and detailed fleet status avoids repeated ownership scans.

### Fixed

- **Detailed fleet status avoids repeated ownership scans.** The
  detailed status matched every child to its task by scanning all tasks, and
  every task to its owner by scanning all children. Ownership is now looked
  up once per turn and the rendered text is unchanged. In an isolated
  benchmark of pure context preparation, without database, engine or model
  time, the observed median for 1,000 children with 1,000 tasks was 0.48 ms
  instead of 1.20 ms. For fleets of ten children or fewer, observed median
  increases were at most 0.7 µs; this is not a worst-case bound.
- **Approval navigation returns to the requester's latest output.** Opening
  an approval's requester from another agent, or denying an approval, now
  restores its live transcript when an older history window was displayed.
  New replies stay visible, and a pending older page cannot switch it back.
- **A changed requirement is no longer mistaken for a duplicate memory.**
  Admission treated notes that shared most of their words as one note, and
  skipped negations, short words, case and punctuation, so "do not run cargo
  fmt" matched "run cargo fmt", and "under 40 lines" matched "under 60 lines".
  The new note was rejected as redundant, or silently replaced the old one.
  Only a note with the same text, apart from surrounding whitespace, is now a
  duplicate; any other note is admitted beside the original, which stays
  current, and naming the original in `supersedes` still replaces it. A note
  that differs only in case or punctuation is now kept as a second memory.
- **Recalled memory shows its origin and date.** Ordinary memory lines name
  the recorder category and creation date, and the prompt says current
  instructions take precedence over recalled context. Imported and unknown
  origins stay distinct; repository notes keep their untrusted file labels.
- **An agent's replies stay in the terminal UI while its children are busy.**
  Every event of a child counted toward its parent's 5,000-event window, so a
  busy child trimmed the parent's own replies out of the transcript and left
  only "earlier history trimmed". A child's events now count only when they add
  a cell to the parent's transcript, such as its spawn or its result.
- **Sending a message returns to the latest output.** In an older history
  window, new replies, including the answer to the message just sent, were
  held off screen until Ctrl+End. Sending a message to the viewed agent now
  leaves the older window and follows the conversation, as Ctrl+End does.

## 1.0.3

Images on a message: attach screenshots and other images in the terminal UI
or with `aether send --image`. Also a fix for retried image sends and a
steadier browser test.

### New

- Attach screenshots and other images to a message. In the terminal UI,
  paste or drop an image file's path, type `/image PATH`, or press Alt+V for
  the image on the clipboard; each shows as a chip above the draft until the
  message is sent. From a script, `aether send --image FILE` (repeatable).
- Claude Code and Codex agents receive the image itself, as do API models
  that take images. The message text always names the stored file, so an
  agent on a model without vision can still open it.
- PNG, JPEG, GIF and WebP; up to 10 images of 3.75 MiB each and 10 MiB per
  message, checked by the daemon with a clear error.
- Protocol 19: `send_message` takes `attachments`. A text-only message is
  unchanged on the wire. The daemon serves 17, 18 and 19; older clients,
  the phone web app among them, read each image as a line of text.
- The phone web app does not attach images yet.
- Messages that wait in an agent's inbox together are handed over one
  message's worth of images at a time (10 images, 10 MiB), so several
  screenshots sent in a row cannot outgrow one engine input line. The rest
  follow as soon as the engine takes input, or open the next turn.
- An API model's request carries the newest 20 images and 15 MiB of them,
  counted across the whole history; older images become a line naming the
  stored file. A long conversation with many screenshots therefore stays
  inside the provider's request size.
- The generated Python and TypeScript types of an `image_ref` content block
  carry its `id`, `media_type`, `size_bytes` and `name`.

### Fixed

- **A retried image send is answered from its receipt first.** A send whose
  idempotency key was already delivered, to one agent or a subtree, and a
  `resolve_submission` for it are answered before any image is decoded or
  stored. A retry no longer needs the attachment directory, a resolution never
  writes to it, and a send the daemon refuses leaves no image file behind.
- **The phone web app's layout test no longer times out on CI.** The mock
  daemon's shutdown waited on a connection the browser had opened and never
  used. It now closes every connection.

### Before updating

- **Store schema 19.** No table changes: the step marks a store whose records
  may hold image references, which an older binary cannot read. The migration
  is one way: Aether saves the old database first (see
  [migration backups](https://github.com/mengsig/aether/blob/main/docs/migration-backups.md)), and a binary that predates
  schema 19 refuses to open a migrated database. To go back, stop the daemon
  and restore that backup with the older binary; changes made since are lost.

## 1.0.2

A fix for sign-in on machines where `~/.config/aether` was created writable by
its group, as it is under the `umask 002` that Ubuntu gives a user.

### Fixed

- **Signing in works when the credentials directory is unsafe.** Aether refuses
  to read stored credentials from a directory that its group or anyone else can
  write to. That refusal used to mark every provider as not connected, so the
  TUI kept asking for `/login` although `claude auth status` said you were
  signed in. Providers that sign in through their own CLI (Claude Code, Codex)
  or need no credentials (Ollama) now work regardless; a stored or environment
  key still comes first when one can be read.
- **Aether creates its directories private.** The config directory and the ones
  beneath it are made with mode `0700` whatever the umask. An existing unsafe
  directory is still refused for stored credentials, and the error now names
  the `chmod 700` that repairs it.
- **The reason a provider is not connected is shown.** `aether auth`, the login
  view and the TUI's header report what the daemon found, and a CLI sign-in
  check reads as the program and its outcome. `aether doctor` warns about an
  unsafe credentials directory.

## 1.0.1

Remote control: drive Aether from a phone, and from another machine over SSH.
See [Remote control](https://github.com/mengsig/aether/blob/v1.0.1/docs/remote.md) and [phone setup](https://github.com/mengsig/aether/blob/v1.0.1/docs/remote-setup.md).
Also a fix for failed tool calls in view modes that hide tools.

### New

- **Phone web app.** The daemon serves an installable web app from a loopback
  listener that is off until `aether remote enable`. `tailscale serve` (or your
  own HTTPS proxy) puts it on your tailnet; the app is compiled into the binary.
- **Device pairing and roles.** Each phone is a device with its own key,
  enrolled by a one-time code you confirm at the host. A device is an
  `operator` or an `observer`; `aether remote pair`, `devices`, `rename` and
  `revoke` manage them, and events record which device acted.
- **Push notifications.** Opt-in Web Push tells you that an agent needs you,
  without the agent's words unless `push_previews` is set.
- **Host services.** Path completion, workspace listing and validation, and a
  git summary for the phone's pickers.
- **Guided `aether remote setup`.** Checks Tailscale, enables the listener,
  publishes it and pairs a phone; safe to re-run. `--yes` never waits and
  leaves pairing for `aether remote pair`; `--json` reports without changing
  anything; `--proxy-url` and `--funnel` are the alternatives to a private tailnet.
- **SSH named remotes.** `aether remote add NAME --ssh TARGET`, then
  `aether --remote NAME <command>` runs against that machine's daemon; no
  listener, token or pairing is involved. `aether --local` forces this host.
- **Protocol 18.** The daemon serves 17 and 18; version 17 clients are
  translated. Adds the carrier's challenge frame, `Hello.device` and
  `Welcome.device`, the `forbidden` and `unimplemented` error codes, device
  provenance, and idempotency keys on eighteen commands. See the
  [protocol](https://github.com/mengsig/aether/blob/v1.0.1/docs/protocol.md).
- **Lost acknowledgements are resolved, not guessed.** `resolve_submission`
  tells a client whether a keyed command whose reply was lost ran
  (`executed`, with its original receipt), provably did not (`absent_safe`,
  so it is sent again under the same key) or cannot be accounted for
  (`unknown`). An SSH remote no longer refuses to connect while a send is
  unresolved: it resolves each on connecting, and `aether --remote NAME
  outbox` and the TUI's `/outbox` let you mark delivered, send again or
  discard what the host cannot confirm. See
  [resolving a lost acknowledgement](https://github.com/mengsig/aether/blob/v1.0.1/docs/remote.md#resolving-a-lost-acknowledgement).
- **Configuration.** A `[remote]` section in `config.toml` with `address`,
  `allow_remote`, `max_connections`, `idle_timeout_secs`, `push_previews`,
  `push_subject` and `push_hosts`; defaults are loopback only and no previews.
  See [configuration](https://github.com/mengsig/aether/blob/v1.0.1/docs/configuration.md#remote-control).
- `aether doctor` also checks remote access.

### Fixed

- **A failed tool call no longer floods a view that hides tools.** In a view
  mode whose `show` omits `tool`, such as the built-in `focus`, a failed call
  printed its whole command and output. It is now one row: the tool's name
  (`Command` for a shell call), `failed` and the first line of the error.
  Failed calls with nothing shown between them share the row of the first.
  Modes that list `tool`, the standard mode and Ctrl+T draw failed calls whole,
  as before. See [view modes](https://github.com/mengsig/aether/blob/v1.0.1/docs/tui.md#view-modes).

### Before updating

- **Store schema 18.** Adds the paired devices and push subscriptions, and
  numbers idempotency receipts for the ledger that resolution reads. The
  migration is one way: Aether saves the old database first (see
  [migration backups](https://github.com/mengsig/aether/blob/v1.0.1/docs/migration-backups.md)), and a binary that predates
  schema 18 refuses to open a migrated database. To go back, stop the daemon
  and restore that backup with the older binary; changes made since are lost.
- **Protocol 17 clients keep working.** The daemon serves protocol 17 and 18,
  so a client built against 1.0.0 still connects.
- **Nothing is exposed until you ask.** The remote listener stays off until
  `aether remote enable` or `aether remote setup`.

## 1.0.0

First public release.

### Terminal UI

- `aether` opens a conversation with one agent beside a board of the whole
  agent tree. Send, steer a running turn, queue a message, interrupt, pause
  and resume from the keyboard.
- Inspect tool calls, results, reasoning, token usage and estimated cost, and
  answer approval prompts where you type.
- Themes (`auto`, `dark`, `light`, `mono` and more), reduced motion, ASCII
  glyphs, and a plain line mode for pipes and screen readers.

### Daemon and durable agents

- A local daemon starts on demand and keeps agents running after the terminal
  closes. Agents, tasks, history and artifacts are stored on disk, and agents
  resume when the daemon starts again.
- `aether run` for one task from a script, and commands to list, message,
  pause, resume, interrupt, cancel, replay and report on agents and runs.
- Python, TypeScript, Rust and MCP clients drive the same daemon protocol.
- `aether doctor` checks an installation; `aether diagnose` writes a redacted
  diagnostic directory.

### Engines and model providers

- Claude Code and Codex run as native engines, with their own tools and
  sign-in, coordinated by Aether.
- Aether's own loop runs hosted APIs (Anthropic, OpenAI, xAI/Grok,
  Google/Gemini, Groq, OpenRouter), local servers (Ollama, LM Studio, vLLM,
  llama.cpp) and any OpenAI-compatible endpoint.

### Delegation and worktrees

- Agents delegate tracked tasks to other agents with an explicit model and
  effort, and wait for results without polling a model.
- A delegated agent can work in its own Git worktree; its parent integrates
  the changes after a check for conflicting edits.

### Schedules and watchers

- Recurring schedules, by interval or cron expression, deliver to an agent or
  start a fresh run. One-shot durable wakes, external triggers and signed
  webhooks wake agents too.
- An opt-in GitHub watcher routes authorized mentions, labels and reviews in
  selected repositories to agents.

### Budgets and limits

- Token and turn limits per agent and per scheduled run, and dollar limits
  where the engine reports a cost that can be enforced; a spend limit per run
  that stops new delegation; limits on concurrent turns.
- Approval rules for tool calls. Approvals are off by default.

### Memory

- Scoped, durable memory of facts, decisions and preferences, with
  `aether memory` to browse, pin, forget and restore it, and import of notes
  committed to a repository.

### MCP

- Configured MCP servers reach every agent through the same approvals as
  built-in tools, and `aether mcp serve` exposes Aether itself over MCP.

### Voice

- Optional local dictation and spoken questions in the terminal UI, off until
  turned on. Voice is outside the stable core of 1.0.

### Install and update

- `install.sh` installs a prebuilt, checksum-verified binary without `sudo`,
  for Linux (x86_64 and aarch64, statically linked) and macOS (Apple silicon
  and Intel).
- `aether update` installs a newer release, `--check` only reports one, and
  `--version` installs exactly the release named.

### Known limitations

Approvals and worktrees are not a security sandbox, native Windows is not
supported, and macOS is tested less often than Linux. The full list is in
[known limitations](https://github.com/mengsig/aether/blob/v1.0.0/docs/known-limitations.md).
