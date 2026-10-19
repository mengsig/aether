# User guide

Detailed workflows and behavior beyond the [short README](../README.md).

## Get started

After [installing Aether and an engine](../README.md#install), open a terminal in a project you trust, run `aether`, and type your request.
For sign-in, see the [quick start](../README.md#quick-start); for source builds and version pinning, see [installation](install.md).

Aether starts its daemon on demand and opens the TUI. Use `/login claude-code`
or `/login codex` only if you have not already signed in to that engine CLI.
Choose a model with `/model`, then type your request.
The agent uses the project directory you launched from, not the directory
where the daemon originally started. Relaunching in that directory returns to
its agent; a live agent in another project does not stand in for one here.
Ctrl+O switches workspace without leaving the TUI.

For a one-shot task:

```sh
aether run "Read this repository and explain its architecture" \
  --model claude-code/opus --effort medium

aether run "Review the current diff without editing files" \
  --model codex/sol --effort medium
```

Use `aether doctor`, `aether models`, and `aether providers` to inspect your
actual installation. Available models depend on your CLI, account, and provider.
Aether creates its configuration directory with mode 0700 on Unix. If an existing
credential directory is group- or world-writable, `aether doctor` and API sign-in
status show the repair command, such as `chmod 700 ~/.config/aether`. Aether keeps
refusing stored secrets until it is repaired; Claude Code, Codex, and providers
that need no authentication can still work through their own sign-in methods.
Failed CLI sign-in checks show the program and reason in `aether auth`, `/login`,
and the session's sign-in hint.
For a portable redacted diagnostic directory, run
`aether diagnose --out ./aether-diagnostics`; see the [bundle guide](diagnose.md)
for run selection, contents and redaction limits.

`aether run` exits `0` when completed or waiting for operator input, `1` on
failure, and `3` on cancellation (`2` is reserved for command-line argument
errors). It prints usage and any `--json` result before returning that status.
Continuation and recovery commands printed by the CLI and TUI retain a socket
selected with `--socket` or `AETHER_SOCKET` as a shell-quoted `--socket` argument.
They can be copied into another shell without changing the target daemon. With the
default socket selection, hints use plain `aether`. For paths that cannot
be printed safely, hints ask you to reuse the original socket argument.

When the run delegates, it also prints the subtree usage including descendants;
the JSON result adds `subtree_usage` alongside the root's own `usage`.

Built-in Claude models are Opus 5.5, Sonnet 5.5, Haiku 5.5 and Fable 5.1, on
both `claude-code/...` (native harness) and `anthropic/...` (API). Opus 5.5 is
preferred: `opus` resolves to `claude-opus-5-5`, and automatic Claude selection
uses it; explicit model defaults remain respected. Sonnet 5.5 (`sonnet`) is the
lower-cost option for balanced work and Haiku 5.5 (`haiku`) the cheapest. `fable`
resolves to `claude-fable-5-1`. Claude Code receives these explicit ids rather
than its own floating aliases; `claude-code/default` leaves model selection to
Claude Code itself.

Built-in GPT models are GPT-6 Astra, GPT-6.1 Sol and GPT-6 Luna, preferred in
that order, on `codex` and on the OpenAI API providers: `astra` resolves to
`gpt-6-astra`, `sol` to `gpt-6.1-sol` and `luna` to `gpt-6-luna`.
Automatic Codex selection uses Astra; choose capability first, then cost within
the tier suitable for the task. Sol is the lower-cost option for demanding work
and runs at high effort unless you pass `--effort` (Codex's own default is low);
Luna is the low-cost option for focused work. Explicit model defaults and
budgets are respected. `codex/default` delegates model selection to Codex. This
is selection guidance, not a forced routing policy or a guarantee of identical
accuracy on every task.

Superseded built-in models (Opus 4.6–5, Sonnet 4.6 and 5, Haiku 4.5, Fable 5,
GPT-6 Sol, GPT-5.x and their aliases) are retired: naming one fails with the model to use
instead, rather than silently running a different one.

For GPT-6 API agents with reasoning and tools, use `openai-responses/...`;
Chat Completions cannot provide that combination. Costs shown are
API-equivalent estimates, not subscription bills
([known limitations](known-limitations.md)).

Reinstalling the binary does not upgrade an already running daemon; restart it
when it is safe to interrupt attached work.

## Native harnesses, Aether orchestration

| Execution path | Model loop and workspace tools | Agent coordination |
| --- | --- | --- |
| `claude-code/...` | Official Claude Code CLI | Aether through MCP |
| `codex/...` | Official Codex CLI | Aether through MCP |
| Direct API or local endpoint | Aether's provider adapters and tool loop | Aether |

Native agents retain their harness's own coding guidance, project-instruction
discovery, tools, and automatic compaction. Aether supplies task state and its
coordination surface; it does not replace native Read/Bash with capped Aether
wrappers or impose a default coding/delegation strategy. Native tool catalogs
can differ between CLIs and versions.

Native delegation and scheduling are restricted so coordinated work stays in
Aether's task tree and accounting. Aether's `delegate` requires **both `model`
and `effort`**; neither is silently inherited. This routing is not specific to
the models used in a particular benchmark.

Delegation uses the parent's workspace by default. For concurrent edits,
`delegate` accepts `workspace_mode="isolated"` and an optional
`workspace_root` selecting a Git repository within the parent's scope. The
child receives a separate worktree containing the source's working state at
delegation. Its parent can later call `integrate_workspace` after inspecting
the completed task and child changes; integration checks for conflicting
parent edits. See [delegated workspaces](harness-tools.md#delegated-workspaces).

Native defaults are preserved unless an explicit configuration or Aether's
coordination/security boundary requires otherwise. Authored model profiles
are opt-in for API agents, and the Claude prompt-cache TTL override is unset
by default. See the [engine contract](engines.md).

### Providers

- **Native:** Claude Code and Codex, using their own authentication flows.
- **Direct API:** Anthropic, OpenAI chat completions, and OpenAI Responses.
- **Local/compatible endpoints:** Ollama, vLLM, LM Studio, llama.cpp, and
  configured OpenAI-compatible services such as OpenRouter.

Use provider-qualified identifiers: `claude-code/sonnet` and
`anthropic/claude-sonnet-5-5` are different authentication/billing paths.
API credentials can be configured with `aether login anthropic` or
`aether login openai`. Endpoint overrides and additional providers belong in
`providers.toml`. A compatible endpoint does not imply every optional model
capability is supported.

## A terminal you can supervise from

The interface is a conversation with one agent at a time: its transcript
fills the screen, and a board of the whole agent tree, with an inspector for
the agent under the cursor, docks on the right. Ctrl+G goes to the board and
back; Ctrl+B shows or hides it. Enter on a board row
views that agent; everything you type goes to the agent you are viewing.
While the board has focus, Shift+X confirms ending the cursor's agent and its
live descendants; Shift+D archives just that agent (confirming stop first if
live), and restores archived rows shown with `.`. History stays. Shift+P
pauses/resumes the subtree; Shift+I interrupts only its current turn. Removing
a root or the viewed agent asks first; removing the viewed agent switches the
chat to another available agent.

| Action | Control |
| --- | --- |
| Send; while the agent runs, steer it | Enter |
| Queue for after this turn / newline | Tab / Shift+Enter, Alt+Enter or Ctrl+J |
| Attach an image: a file / the clipboard | paste its path or `/image PATH` / Alt+V |
| Go to the agents / show or hide them | Ctrl+G / Ctrl+B |
| View the parent agent | Alt+↑ |
| Interrupt the current turn | Esc |
| Shortcuts / commands | `?` / `/` |
| Details (full output, reasoning) / find | Ctrl+T / Ctrl+F |
| Search message history | Ctrl+R |
| Model and effort / provider usage | `/model` / `/usage` |
| Theme, motion and mouse | `/settings` |

Themes are `auto` (light or dark from the terminal), `dark`, `light` and
`mono`; reduced motion, ASCII glyphs and a plain line mode are available.
Mouse capture is on by default: wheel scrolls the pane under the pointer;
drag to select & copy within one pane. Shift+drag uses terminal selection;
`/mouse off` disables capture. See the [TUI guide](tui.md) for every key and
command.

A message can carry screenshots and other images next to its text: paste or
drop the file's path into the composer, type `/image PATH`, or press Alt+V for
the image on the clipboard. Each shows as a chip above the draft until you
send. PNG, JPEG, GIF and WebP are accepted, up to 10 images of 3.75 MiB each
and 10 MiB per message. See [images in the TUI guide](tui.md#images).

The notification center collects what agents and services such as Archon
need you to see: questions, blockers, decisions, outcomes and plain news. The
bottom right of the screen counts unread notifications and marks questions
and blockers; `/notifications` opens the panel, newest first, where you can
read one, pick one of its answers or type your own, mark it read, resolve it,
or jump to the chat of the agent that posted it. An answer resolves the
notification and reaches the agent that posted it as an inbox message;
a service reads it from the daemon's events. Notifications survive restarts
and are shared by every TUI on the daemon. Agents post with the
[`notify`](harness-tools.md) tool, services with
[`post_notification`](protocol.md#notification-center); see the
[TUI guide](tui.md#notification-center) for its keys.

Optional [local voice](voice.md) supports dictation and spoken questions;
recording and speech are off by default.

Closing the TUI (`/quit`, Ctrl+D on an empty composer, or Ctrl+C once there is
nothing left to close or interrupt) disconnects the client and leaves agents
running. `aether stop` stops the daemon and
interrupts its attached processes. The next start retries interrupted turns
whose tools are safe to retry. Turns with uncertain tool effects wait for
inspection and `aether resume AGENT_ID`. These are deliberately different actions.

## Work that starts from events

### GitHub

The opt-in GitHub watcher checks selected repositories for authorized matching
mentions, labels, and reviews. Add a repository, then mention `@Aether` in a
new issue, PR, comment, or submitted review. Aether gives each thread its own
managed checkout and agent:

```sh
aether github-watcher add owner/repo
aether github-watcher status
```

Custom routing is available when needed:

```sh
aether github-watcher add owner/repo --agent AGENT_ID

# Alternatively, let each issue/PR have its own agent.
aether github-watcher add owner/repo \
  --spawn-role reviewer --spawn-objective "Review the requesting thread."

# Only these people may dispatch; each still needs write access unless it
# is an operator login.
aether github-watcher allowed-users owner/repo alice bob

# These people's own requests carry your authority. Off unless you set it.
aether github-watcher operator-users alice

```

Automatic and custom thread routing reuse a usable thread agent and replace a
paused or terminal one. The daemon polls GitHub (120 seconds by default); models
do not poll it. Watching is off until a repository is added; activation does not
backfill old requests.

A GitHub request reaches an agent as external content from a collaborator
with write access, never as an instruction from you. The one exception is
opt-in: a login named in `operator-users` speaks with your authority, for text
the watcher verified that account wrote and nobody else edited. Such a login
may dispatch on every watched repository whose allowlist admits it, even
without write access there. Such an account is as powerful as you are, so name
only accounts you would hand your terminal to. It answers no approval prompt
and changes no approval, network, or sandbox setting. A request ignored from
an operator login, or from someone without write access, leaves a one-line
notice on the thread's agent or in `aether events` instead of silence.
See [routing, authorization, and setup](github-watcher.md).

### Timers, webhooks, and clients

Use `aether schedule --help` for recurring delivery and fresh-run schedules,
or `wait_for` for a one-shot durable wake. The agent-facing scheduling tool
does not yet expose every CLI scheduling mode. CLI schedule dates show whole
seconds in UTC; JSON preserves timestamp precision.

A tick that finds a delivery schedule's target paused is recorded as skipped
("target is paused") and dropped. It does not count towards `--max-deliveries`,
and nothing is replayed after `aether resume`; the same holds for a tick that
finds the target busy under the default policy (`--queue-one` holds one trigger
instead). `aether schedule list` ends a line with the number of skipped
occurrences among the schedule's newest 1000 recorded, when there are any; `N+`
means the scan was full, so older ones may exist too. `aether schedule history`
lists them with their reasons.

`aether serve` bridges signed incoming webhooks and outgoing event
subscriptions. Use the `[serve]` section of the configuration template.
Python, TypeScript, Rust, and MCP clients can drive the same daemon protocol;
there is no separate privileged orchestration application required.

External trigger JSON objects may include a top-level string `summary`, for
example `{"summary":"Build passed","build_id":42}`. A nonblank summary becomes
the TUI's one-line notice; Ctrl+T keeps the bounded JSON payload preview available.
The full payload still reaches the agent. See [trigger presentation](tui.md#view-modes).

## Inspect, recover, and automate

```sh
aether status --json
aether agents
aether tasks --json
aether events -f
aether send AGENT_ID "Please explain the current blocker."
aether send AGENT_ID --image shot.png --image log.png "What is wrong here?"
aether pause AGENT_ID
aether resume AGENT_ID
aether replay AGENT_ID --json
aether runs list
aether runs report RUN_ID --md
aether runs export RUN_ID --out run.aether
```

Whether a command starts the daemon depends on the command. `aether`, `tui`,
`run`, `new`, `models`, `providers`, `auth`, `github-watcher` (even `status`) and
`remote setup` start one when none is running. The other commands that talk to
the daemon, such as `status`, `agents`, `tasks`, `send` and `schedule`, fail with
"no daemon" instead. `doctor`, `schema`, `diagnose`, `bridge` and `voice` start
none. `aether --help` says the same.

Wherever a command takes an ID, a unique prefix of it works as well, with or
without its `agt_`/`run_`/`tsk_`/`apr_`/`sch_` part. An agent or a schedule
can also be named by its name when no other live one has it, and `@last` names
the newest agent of the current run (or, where a run is expected, that run).
`@viewed` names the agent a TUI on this machine views, or its run; with several
TUIs open, the one whose view changed last. The agent in a memory scope such as
`agent:planner` or `subtree:@viewed` is named the same way. When a prefix or
name matches more than one, the command refuses and lists them.

Plain `aether events` shows tool-result text and error markers, plus custom
event names and brief payloads. Use `--json` for complete event records.

`aether send --image FILE` attaches an image and may be repeated; with images
the text is optional. The same limits apply as in the TUI, and a file that is
not a PNG, JPEG, GIF or WebP, or is too large, is refused before anything is
sent.

For a parsed command with `--json`, command errors print one JSON object on
stdout and a human `Error: …` on stderr, with the existing exit code:

```json
{"error":{"code":"not_found","message":"daemon refused: NotFound: unknown agent","socket":"/path/to/aetherd.sock"}}
```

`code` preserves daemon error codes such as `invalid_argument`, `not_found`,
and `invalid_transition`. Client failures use `daemon_unreachable`,
`incompatible`, `unauthorized`, `timed_out`, `daemon_startup`, `io_error`,
`protocol_error`, or `invalid_argument`; other command errors use `internal`.
`message` includes the error's context and causes, and `socket` is the selected
socket (including `--socket` or `AETHER_SOCKET`). Success objects, run outcome
objects (including failed or cancelled runs), and doctor reports are unchanged.
Streaming commands may already have emitted records before a terminal error
object. Argument parsing errors retain Clap's stderr usage output and exit code 2.

Run exports preserve evidence; imports do not automatically restart work.
`aether task --owner AGENT_ID` requires a live agent without a current task.
Agents created by `aether run` already own a task; create a taskless agent with
`aether new` or `/new` in `aether tui`, then use its ID as the owner.
Check `aether COMMAND --help` for each command's exact options. Generated
[Python](../clients/python/README.md) and [TypeScript](../clients/typescript/README.md)
clients are checked against the Rust protocol schema in CI.

## Safety and honest limits

Use `aether approvals` to list pending approvals with their full IDs, agent IDs,
tools, argument summaries, side-effect classes and ages. Filter with exactly one
of `--agent AGENT`, `--subtree AGENT` (including that agent), or `--run RUN`.
`--json` returns an array of approval request objects, ordered by request time
then ID; `requested_at` supplies the stable timestamp instead of a changing age.

Answer one ID with `aether approve APPROVAL` or `aether deny APPROVAL`. To answer
a batch, use `aether approve --all-pending --run RUN` (or `deny`, with any of the
three scopes). The CLI confirms the selected count; `--yes` skips confirmation.
Only that snapshot is answered, so approvals arriving later wait for another
command. Concurrently answered IDs are reported as already resolved; other
failures stop the batch and report how many answers succeeded. Batch answers
cannot be combined with `--remember`, `--tool-wide`, or `--everywhere`.

**Approvals are off by default.** Do not run unfamiliar repositories or
untrusted requests in a privileged workspace. `--no-yolo` requires approval for
external side effects only; local file writes do not prompt. Before sending
work to an agent, use `aether set AGENT_ID --approval
idempotent_mutation,mutation,external` to also require approval for local
mutations. Approval prompts are not a general filesystem sandbox.

- Native shell/file operations do not all pass through Aether's tool guards
  or resource leases. Isolated Git worktrees avoid ordinary shared-directory
  edit collisions, but are not a security boundary for native tools.
- The container backend covers Aether shell execution, not the whole native
  Claude/Codex harness. Native-engine/container combinations are rejected.
- `wait_for` preserves reported native background jobs in their owning
  session, with no model polling. That session occupies an engine slot and is
  not restart-durable. Other turn-ending actions are not covered by this path.
- Message delivery/acknowledgment is not proof that the model answered a
  question or followed an instruction. Inspect outcomes, not just status.
- Context occupancy and cumulative token usage are different measurements.
  Native Claude Code turns are priced at API list prices and Codex turns are
  estimated from them, not verified subscription charges. Unknown usage or
  pricing is shown as unknown.
- Agent waits are event-driven, but maintenance, GitHub checks, and interface
  refreshes can still use timers. Zero waiting tokens does not mean zero
  operating-system wakeups.
- Remote TCP access has no built-in TLS. Use an SSH tunnel; do not expose a
  privileged daemon directly to an untrusted network.

The release does not claim completion of every historical roadmap item or
reliability target. See [known limits](known-limitations.md).
