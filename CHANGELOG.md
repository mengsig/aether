# Changelog

What each release of Aether brings, newest first. Aether follows
[Semantic Versioning](https://semver.org/spec/v2.0.0.html). The section of a
release is also the text of its GitHub release.

## Unreleased

### Images on a message

- Attach screenshots and other images to a message. In the terminal UI,
  paste or drop an image file's path, type `/image PATH`, or press Alt+V for
  the image on the clipboard; each shows as a chip above the draft until the
  message is sent. From a script, `aether send --image FILE` (repeatable).
- Claude Code and Codex agents receive the image itself, as do API models
  that take images. The message text always names the stored file, so an
  agent on a model without vision can still open it.
- PNG, JPEG, GIF and WebP; up to 10 images of 3.75 MiB each and 10 MiB per
  message, checked by the daemon with a clear error.
- Protocol 18: `send_message` takes `attachments`. A text-only message is
  unchanged on the wire, and protocol 17 clients keep working.

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
