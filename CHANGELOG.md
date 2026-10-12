# Changelog

What each release of Aether brings, newest first. Aether follows
[Semantic Versioning](https://semver.org/spec/v2.0.0.html). The section of a
release is also the text of its GitHub release.

## 1.0.1

Remote control: drive Aether from a phone, and from another machine over SSH.
See [Remote control](https://github.com/mengsig/aether/blob/v1.0.1/docs/remote.md) and [phone setup](https://github.com/mengsig/aether/blob/v1.0.1/docs/remote-setup.md).

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
- **Store schema 18.** Adds the paired devices and push subscriptions, and
  numbers idempotency receipts for the ledger that resolution reads. The
  migration is one way: Aether saves the old database first (see
  [migration backups](https://github.com/mengsig/aether/blob/v1.0.1/docs/migration-backups.md)), and a binary that predates
  schema 18 refuses to open a migrated database. To go back, stop the daemon
  and restore that backup with the older binary; changes made since are lost.
- **Configuration.** A `[remote]` section in `config.toml` with `address`,
  `allow_remote`, `max_connections`, `idle_timeout_secs`, `push_previews`,
  `push_subject` and `push_hosts`; defaults are loopback only and no previews.
  See [configuration](https://github.com/mengsig/aether/blob/v1.0.1/docs/configuration.md#remote-control).
- `aether doctor` also checks remote access.

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
