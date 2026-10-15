# Engines

An **engine** is an official CLI that runs inference under the person's own
subscription login. Aether ships two, Claude Code and Codex, and the `Engine`
trait is the contract any other CLI implements — Gemini CLI, OpenCode, your
own. This page is that contract, with a worked third-party example.

An engine owns the model loop and its own built-in tools. Aether stays the
authority on tasks, delegation, waits, events, approvals and cost: it hands
the engine one prompt per turn, exposes its harness tools over MCP, records
everything the engine streams back, and resumes the engine's session next
turn. An engine keeps its conversation session, while Aether supplies the shared
dynamic state on every turn without supplying a default coding policy. Native
engines own automatic compaction. Explicit checkpoints and cross-provider
handoffs still validate a checkpoint before replacing a session.

The optional `[model].profiles` coding guidance applies only to API models.
Native Claude and Codex agents retain their own guidance even when profiles
are enabled globally; task instructions and Aether orchestration tools remain
available.

For a one-shot agent with an active task, a nonblank native final answer
completes that task directly. Aether does not spend another native turn asking
for `complete_task`. An outstanding wait still takes precedence, and long-lived
agents return to waiting for input without implicitly closing their task.
Blank or unanswered turns retain bounded recovery rather than being reported
as successful completion. Explicit Aether task-completion tools remain available.

## Which tools Aether owns

Keep each engine's native web search, fetch, file, shell, and other useful
capabilities. Different native tool catalogs are expected; Aether does not
need replacement tools merely to make those catalogs identical. Web access
must follow the agent's configured network scope and the engine's permissions.
Online Codex turns inherit the native search mode, including an operator's
disabled setting. Aether disables search for offline or isolated summary turns.

Use Aether tools for operations whose state must be shared and visible across
providers: delegation, tasks, steering, waits, scheduling, and durable memory.
Native delegation, agent messaging, workflows, and scheduling are disabled
because they create state outside Aether’s tree, task lifecycle, limits, and
event accounting. Claude’s native background-shell controls (`TaskOutput` and
`TaskStop`) and code-review rendering (`ReportFindings`) remain available.
Aether supports one-shot durable wakes and recurring schedules; the agent tool
does not yet expose CLI spawn-target scheduling. Compaction is a
separate restricted invocation; its tool restrictions do not define what an
ordinary agent should be able to use.

A detected native delegation bypass stops the engine and interrupts the Aether
turn. The agent and task remain available for resumption; a durable
`engine.interrupted` event records the reason. An adapter reports this with
`EngineOutcome.interrupted`, alongside its error detail. Ordinary engine
failures retain their existing failure behavior.

For an explicitly network-denied scope on Linux, Codex runs commands in a
`codex exec-server` inside `bwrap --unshare-net`: loopback works; egress and
DNS have no route, and seccomp allows only IP, netlink and connected unix-pair
sockets (host unix or vsock listeners could proxy). Each such turn's input tells
the model only loopback is reachable; a later network-allowed turn on that thread
retracts it. Playwright's default `chrome-headless-shell` works; full Chrome's
unix-socket process singleton aborts, exactly as in Claude's sandbox.
These turns need Codex 0.159.0 or newer (its experimental `environment/add`
and per-turn `environments`) plus `bwrap` and `socat`, or they fail; other
platforms keep Codex's network-disabled workspace sandbox. Legacy `codex exec`
cannot enforce that scope and refuses the turn.

Claude uses its native sandbox with a strict empty network allowlist,
no unsandboxed retries, and isolated settings/MCP configuration so inherited
exclusions cannot bypass the restriction. These Claude turns require version
2.1.219 or newer and the native sandbox dependencies; unavailable sandboxing is
a visible startup failure. Ordinary network-enabled turns retain native settings.
See [Claude's sandbox documentation](https://code.claude.com/docs/en/sandboxing)
for the Linux `bubblewrap`/`socat` dependencies and supported platforms.

Codex's Aether MCP server uses `default_tools_approval_mode="approve"` so
calls reach the authenticated bridge without a second native approval gate.
The daemon still enforces the agent's tool visibility, scope, and approval
policy before executing a bridge call. This server-specific setting does not
change native tool permissions or approval settings for other MCP servers.
See the [Codex configuration reference](https://learn.chatgpt.com/docs/config-file/config-reference)
for the native MCP approval setting.

## Claude context windows

A context window is how much a model can work with in one request:
instructions, conversation history, tool results and its reply. Aether keeps
one figure per model in its registry and uses it to show how full an agent's
context is.

| Provider kind | Context window Aether assumes |
| --- | --- |
| Claude Code (`claude-code`) | 200,000 for every built-in model, including `default`. |
| Codex (`codex`) | 272,000 for every built-in model. |
| Hosted and local APIs | The per-model limit in the registry; `aether models` prints it. |

For Claude Code models, 200,000 is the figure Aether divides by when it shows
a context percentage. It is not a cap on the CLI session: Claude Code chooses
its own window, and its
[model documentation](https://code.claude.com/docs/en/model-config#extended-context)
says recent models can already run with a 1M window without a suffix.

Aether records the context token count Claude Code reports and divides it by
the registry figure of the agent's model. It does not read the window the CLI
reports, so the percentage in the TUI and in `inspect_state` is relative to
Aether's figure both before and after a turn. Aether does not set
`CLAUDE_CODE_DISABLE_1M_CONTEXT`, and it passes a `[1m]` model id to
`claude --model` unchanged.

### Use the 1M window

No built-in Claude Code model has a 1M window, there is no alias for one, and
Aether never turns it on by itself. To get one, add this to `providers.toml`:

```toml
[[providers.claude-code.add_models]]
id = "claude-opus-5-5[1m]"
name = "Claude Opus 5.5 (1M context)"
capabilities_from = "claude-opus-5-5"
capabilities = { context_window = 1000000 }
```

- **Where the file is.** `providers.toml` is in Aether's
  [configuration directory](configuration.md). `aether doctor` prints that
  directory on its `config` line, and reports a `providers.toml` it refuses.
  A new installation's template `providers.toml` carries this entry commented
  out; an existing file is never rewritten.
- **What the entry means.** It adds the model `claude-code/claude-opus-5-5[1m]`.
  `capabilities_from` copies the capabilities of a model the provider already
  lists (here the list price used for cost figures, the 128,000 output limit,
  and effort, thinking, vision and tool support), and `capabilities` states
  only what differs. A field stated there replaces the copied value whole, and
  an unknown field is refused. The built-in `claude-opus-5-5` and its `opus`
  alias stay at 200,000.
- **Restart the daemon.** It reads `providers.toml` when it starts: run
  `aether stop`, then `aether`.
- **Select the model.** `aether models` now lists it. Use
  `aether run "Your task" --model 'claude-code/claude-opus-5-5[1m]'`, or
  `/model claude-code/claude-opus-5-5[1m]` in the TUI, or name it as
  `model.default` in `config.toml`. Quote the id in a shell: the brackets are
  pattern characters there.

You can also ask an agent to make this edit for you; it is never applied
without an explicit request.

With the entry selected, Aether passes `claude-opus-5-5[1m]` to
`claude --model` and measures context against 1,000,000, so 75% means 750,000
tokens instead of 150,000. Nothing else changes: compaction of a native
session stays with the CLI (see below).

Whether the CLI grants the larger window, and how its use is charged, depends
on your account and plan; keeping more history also spends more of a plan's
allowance on every turn. Only `claude-opus-5-5[1m]` was observed working on a
real account. An entry for `claude-sonnet-5-5[1m]` or `claude-fable-5-1[1m]`
would be written the same way, but those ids were not tried on an account:
they rest on Claude Code's documentation of the `[1m]` suffix. That
documentation lists no `[1m]` variant of Haiku 5.5.

### Who compacts

Compaction replaces older conversation history with a summary. Who does it
depends on who runs the model loop.

| Path | Who compacts | When | What you can configure |
| --- | --- | --- | --- |
| Native Claude Code | Claude Code, inside its own session. | Its own automatic policy. | Claude Code's settings, for example its auto-compact window (`autoCompactWindow`, `--autocompact`, `CLAUDE_CODE_AUTO_COMPACT_WINDOW`). |
| Native Codex | Codex, inside its own session. | Its own automatic token threshold. | Codex's settings, for example `model_auto_compact_token_limit`. |
| Aether's direct API loop | Aether asks a compactor model for a structured checkpoint, checks it, and folds the history itself. | At 75% of the model's window (or when history had to be trimmed), and after a completed task on a long-lived agent with at least six new transcript entries. | `model.compactor` and `model.compactor_effort` in `config.toml`. The 75% threshold is fixed. |
| `/compact`, `aether compact`, and an agent's `request_checkpoint` | The same Aether checkpoint, on any agent. The compactor model writes the summary: `model.compactor` when it is set, otherwise a model of the agent's own provider, which for a native agent is a model of the same CLI. A Claude Code or Codex compactor writes it in a fresh, isolated CLI session; this is not the CLI's own `/compact`. An API compactor writes it with an API request, for a native agent too. A native compactor cannot run for an agent in a container backend, which needs an API compactor. | Only on request, before the agent's next turn. | The compactor model and effort. |

On a native session Aether still records context usage, but usage never
requests an Aether summary or a task-boundary checkpoint, and automatic
requests stored by an older version are ignored. Explicit checkpoints and
handoffs to another provider still run. After a successful checkpoint a native
agent continues in a new CLI session started from Aether's recorded history,
so state that only the old CLI session held is not carried over.

The native rows describe the CLIs' own behaviour and are not tested by Aether:
see Claude Code's
[context window guide](https://code.claude.com/docs/en/context-window) and
[environment variables](https://code.claude.com/docs/en/env-vars), and the
[Codex configuration reference](https://learn.chatgpt.com/docs/config-file/config-reference).

## Lean host context

Claude Code loads the operator's skill listing and every user, plugin and
claude.ai MCP server into each session. An agent created with
`config.host_context: "lean"` leaves those out: its Claude Code launch adds
`--disable-slash-commands` and `--strict-mcp-config`. The default, `"full"`,
launches exactly as before. Codex and API models ignore the setting.

A lean session keeps CLAUDE.md files, hooks, settings, permissions, plugins,
every built-in tool and Aether's own MCP server. Measured with one operator's
setup on Haiku, an agent's first prompt fell from about 26.6k to 16.6k tokens
(38%). A delegated child inherits
its parent's value unless `delegate` sets `host_context`; set `"full"` for a
task that needs a native skill or another MCP server.

Hooks and settings stay on ordinary online turns. Network-denied turns already
drop operator settings and hooks with `--setting-sources ""` and pass
`--strict-mcp-config`, so lean only adds `--disable-slash-commands` there.
Summary and compaction turns already pass both flags and are unchanged.

## Native hooks and isolation

Ordinary network-enabled turns inherit native hook configuration. Aether disables
hooks for isolated summary invocations and explicitly network-denied turns:
hooks can perform I/O outside the sandbox. Scope and approval enforcement remain
Aether responsibilities; other native setting overrides are still being audited.

## Codex credentials and trust boundary

The Codex model process retains its provider credentials for authentication.
On Codex 0.162.0 or newer, the app-server adapter excludes provider environment
names matching `[A-Za-z0-9_-]+` from native commands after inspecting the raw
config layers. Legacy `shell_environment_policy.exclude` or `include_only`
arrays, failed config inspection, and older or unknown versions retain native
policy and emit a visible notice. Combining legacy arrays with new filters can
discard operator exclusions.

The `codex exec` fallback has no safe config-layer inspection, so it keeps native
policy without adding provider filters. It emits a visible notice once per
session during the engine's lifetime when provider environment names remain
and native commands are enabled. Summary invocations disable commands and hooks.
Its commands can therefore inherit provider credentials; this gap remains open
([known limitations](known-limitations.md)).

Environment filtering is not a credential isolation boundary. `danger-full-access`
turns can read same-UID process environments and files, including native auth
and Aether configuration. Operator hooks on network-allowed turns retain the
model process environment, and operator `shell_environment_policy.set` can
reintroduce values after filtering.

## Codex prompt rules are answered by Aether, not refused natively

Codex reads the operator's `~/.codex/rules/*.rules` on every turn. A rule
with `decision="forbidden"` is a native refusal under any approval policy
and never reaches Aether. A rule with `decision="prompt"` (the shipped
default marks `gh pr merge` and `gh auth login|logout|refresh` that way) is
where the native policy and Aether's meet:

- Under `approvalPolicy never` Codex refuses the command itself, before any
  request: `approval required by policy, but AskForApproval is set to
  Never`, even with `danger-full-access` (verified against codex-cli
  0.154.0). Aether never sees it; the model reads it as a rejection. This is
  what the offline bypass turn, the summary-only turn, and the legacy
  `codex exec` adapter (`--dangerously-bypass-approvals-and-sandbox`) still
  do.
- Under `approvalPolicy on-request` the same rule raises
  `item/commandExecution/requestApproval` (reason "`cmd` requires approval
  by policy"). The adapter turns it into a permission event on tool `bash`
  and the runtime decides it exactly like a call to an Aether-owned tool:
  a gated agent's remembered rule or a person; a bypass agent's policy gates
  nothing, so the prompt is granted. `accept` runs the command inside the
  same sandbox the turn already had; `decline` completes the item as
  `declined` and the model reads "Rejected by user".

The bypass-with-network turn uses `on-request` rather than `never`, so an
ungated agent can run a prompt-covered command such as
`gh pr merge` under Aether's policy instead of a blind native refusal. That
is a deliberate broadening the operator accepted: *every* native prompt rule,
not only merge, is answered by the agent's Aether policy on such turns, and
an empty `approval.require_for` grants all of them. Nothing in Aether policy
encodes a "merge only when green" condition; that stays a workflow concern.
To keep a prompt-covered command refused for bypass agents, change the
native rule to `decision="forbidden"`, or gate the agent (`approval.require_for`
non-empty) and write an Aether deny rule for the command.

The offline bypass turn deliberately keeps `never`: under `on-request` a
model may ask to escalate a command (`with_escalated_permissions`), and an
accepted escalation runs the command outside the sandbox - with network and
writes beyond the writable roots (verified). An ungated agent would grant
that to itself, so offline bypass turns forgo prompt-rule commands instead.
Related limitation, pre-existing and unchanged: on a *gated* offline turn the
person answering an escalation or a `permissions` request can lift the
sandbox for that command; the approval card carries the engine's reason.

## Native reasoning effort

Claude and Codex choose their own effort when the agent has no explicit setting
and the model declares no `default_effort`. Codex adapters omit both the
command-line effort override and the app-server turn effort field in that case.
Explicit agent effort still passes through, independently of registry
capability flags, and always beats a model default. GPT-6.1 Sol declares
`default_effort = "high"` (Codex's own default is low), so `codex/sol` launches
at high unless asked otherwise; the default is applied per turn, never written
to the agent's config.

Invocation snapshots record the effort the turn launched with, or `None` when
native selection is used. An omitted override does not prove which effective effort the
native model used; comparisons must capture that separately.

## Claude prompt-cache tier

Claude Code keeps its native cache settings unless `[model] prompt_cache_ttl`
explicitly selects `"5m"` or `"1h"`. An explicit setting passes through as
`CLAUDE_CODE_PROMPT_CACHE_TTL`; omission adds no environment override.

Anthropic bills a 1-hour cache write at twice the input price, against 1.25x
for a 5-minute one. Claude Code reports the split per request, and Aether prices
each tier at its own rate wherever it prices tokens itself: list-price costs,
API-equivalent estimates, and tokens a closing receipt left out.

## How many engine turns run at once

`[scheduler] max_concurrent_engine_turns = N` caps simultaneous engine turns
across the daemon. An engine turn is a whole CLI session and burns at a normal
single-agent rate, so N of them cost N times that; the cap is the direct lever
on burn rate. A turn on an API model does not count. Only a turn
actually running on an engine holds a slot - an agent asleep waiting on its
children holds none - so a subtree cannot deadlock itself against the cap. A
held-back agent stays ready and is admitted in the scheduler's usual order,
oldest first among equals - and among engine candidates that order is wait
time, not which engine is the emptier, since they all queue for the one pool.
A turn that parks on a resource lease gives its slot away and queues for
another when the key frees, so the cap bounds concurrency rather than only
admission. Unset means no engine cap.

`aether status`, the TUI's `/usage` view and `inspect_state` all report the count
against the cap, and a `delegate` that will queue behind it says so.

The cap (and `run_budget_usd`, the sibling spend cap) is daemon-wide, applied
identically to every run; per-run overrides are a follow-up, not shipped. The run budget is cumulative and never
resets, so a long-lived run that crosses it can never delegate again for the
life of the daemon. A person-addressed turn bypasses the engine cap,
consistent with limits 2-4: messaging an agent directly overrides the
burn-rate bound.

The trade-off that buys: with a cap set, engine turns fill in wait order
rather than spreading across engine providers, so two engines fill oldest-first
instead of alternating - concentrating load, and rate-limit risk, on whichever
engine the oldest candidates happen to sit on.

The cap's liveness cost: a wedged engine session holds its slot until the
agent is cancelled, since there is no per-call timeout on an engine stream.

## The trait

```rust
#[async_trait::async_trait]
pub trait Engine: Send + Sync {
    fn kind(&self) -> EngineKind;
    fn adapter(&self) -> String;
    async fn run(&self, turn: EngineTurn, cancel: CancellationToken)
        -> Result<EngineRun, EngineError>;
    fn gates_permissions(&self) -> bool { true }
}
```

- **`kind`** names the engine. `EngineKind` is an opaque static string
  (`EngineKind::new("gemini-cli")`), so an engine in another crate names
  itself without editing Aether. It keys the runtime's engine table and lands
  in events and snapshots.
- **`adapter`** is the label a snapshot records, `name/version`. Version it:
  it is how a past turn is explained after the CLI changes.
- **`gates_permissions`** says whether the engine asks before acting. Return
  `false` if it cannot, and the runtime refuses to pretend otherwise — agents
  on that engine run without an approval policy rather than with a fake one.

`run` takes an `EngineTurn` — the user prompt and [the images sent with it](#images-on-a-message), `system_prompt` replacing the engine's
built-in base instructions, the session to resume (or a fresh id), model and effort,
the working directory and extra readable directories, the scope's network-access
setting, whether to bypass permissions, the environment variables to strip, and the MCP bridge to
Aether's harness tools. It returns an `EngineRun`: a receiver of
`EngineEvent`, an optional sender for messages steered into the running turn,
and an optional sender for permission and question answers.

### The event contract

Emit `EngineEvent`s as they happen, and end with exactly one `Done`:

| Event | When |
|---|---|
| `Session { id, model, tools }` | first, as soon as the CLI says what it loaded |
| `Delta(StreamEvent)` | live text and thinking, the same shape the API adapters produce |
| `Assistant { content, usage, model }` | one complete assistant message |
| `ToolResult { tool_use_id, content, is_error }` | a tool the engine ran itself |
| `UsageDelta { usage }` | tokens consumed since the last report, as soon as the CLI reports them |
| `RateLimit { payload }` | subscription window, verbatim; Aether normalizes it into provider health |
| `BackgroundTask { id, running }` | the turn is not over while one is running |
| `Permission { request_id, tool, input, side_effect }` | answered on `EngineRun::decisions` with the same id |
| `Question { request_id, prompt }` | answered the same way, with the person's reply |
| `InputAccepted` / `InputRejected` | acknowledge steering, in order |
| `Note(String)` | anything worth logging that is not a turn event |
| `Done(EngineOutcome)` | always last: usage, cost, stop reason, session, final text |

Usage is not reported per message. Claude Code repeats a message's usage on
every content block and Codex reports it once per turn, so adapters emit the
new tokens as `UsageDelta` and leave `Assistant.usage` empty; `Done` carries the
turn's totals, which reconcile what streamed. An engine turn's
`invocation.completed` events therefore carry no usage (zero tokens, unpriced),
and its spend arrives as `agent.usage_updated` with the agent's and the turn's
totals.

Three rules the runtime depends on:

1. **`Done` is always last, and always arrives.** A CLI that dies mid-stream
   still gets a `Done` with `is_error` set, or the turn waits forever.
2. **Cancellation ends the stream.** When the `CancellationToken` fires, kill
   the child and finish; never strand the process.
3. **Never invent.** An unknown line is a `Note`, not a guess. A cost you were
   not told is `None` — Codex reports none, and a fabricated number would
   silently corrupt budgets. Codex turns are priced at OpenAI API list prices
   by the model registry, like Claude Code turns at Anthropic's. `default`
   bills as the model its session reports; a model with no published API
   price stays unpriced.

A harness tool can end the turn under you. When a bridge call parks the agent
(`wait_for`, `delegate` with a wait, `complete_task`, `fail_task`, `ask_user`)
Aether asks you to stop and every later reply says so; stop and emit `Done`.

The ask is stdin close plus, if you offer one, `EngineRun::stop` - a channel
Aether sends the remaining grace on when the turn ends under you. Offer it if
a closed stdin does not stop your CLI: Claude Code exits on the close and sets
it to `None`, Codex ignores the close and turns the signal into
`turn/interrupt`. If your adapter has a wind-down timer of its own, set it from
that `Duration`, never longer. What follows the ask:

- Calls that do not decide the turn's end are still served, so a parallel
  batch does not lose its siblings; one that would decide it again is refused.
- Permission prompts are not put to a person. A rule the operator wrote in
  advance still decides; without one the prompt is denied. For an agent whose
  approval policy gates the call's class, that means a parked-batch sibling is
  refused unless a rule allows it.
- A `complete_task` or `fail_task` park stops the process immediately; a wait
  gives you `engine_park_grace` (`[scheduler] engine_park_grace_secs`, 20s by
  default) before it is stopped, so honour the ask rather than leaning on it.
  Tools you report as backgrounded do not extend it past that knob.

The session is preserved either way and resumes on the next wake.

The stop is a signal, not a sandbox. Your own tools - shell, writes, network -
never traverse the bridge, so nothing about the park interrupts them: they run
until the process does not. Under `bypassPermissions` (an agent with no
approval requirements, or the daemon's `yolo` flag) Claude Code is launched
without `--permission-prompt-tool`, so no prompt is raised at all and the
signal is the whole of the containment. Aether sends `SIGTERM` to your process
group, then `SIGKILL` two seconds later, so the real bound on work after a park
is the grace plus those two seconds plus the reap - and a grandchild you
`setsid` into its own process group is not reached by either signal.

Strip `turn.hidden_env` from the child's environment. The engine drives its
own shell, so a credential left there is one `printenv` away.

### Invocation timing

Each `Assistant` message becomes one `InvocationCompleted` event with a
`duration_ms`. The trait gives no boundary for "the provider started working
on this response" - your adapter is one process wrapped in a channel, not a
provider client, so there is no request/response pair here to time the way
the direct-API path (`turn.rs`) times its own `client.stream(...)` call.
What the runtime measures instead, honestly:

- **The interval since the previous message finished, per attempt.** The
  mark is `StreamState::new`, taken in `drive` right after `engine.run` and
  the initial `steer` call - the start of the current attempt at the turn,
  not of whatever setup ran before `drive`, and not necessarily the outer
  turn's own start if a retry re-entered `drive`. The first `Assistant` of
  an attempt is timed from that mark; every one after is timed from the
  previous message's completion. This is an observed wall-clock span, not a
  model-only figure.
- **It necessarily includes any tool round-trip in between.** If the engine
  called a tool between two assistant messages, that tool's own execution
  time (bridge call, harness work, the engine relaying the result back) is
  inside the next message's `duration_ms`. There is no separate "tool-phase"
  figure to subtract, because the trait does not expose when the engine
  resumed its own model call after the tool answered - subtracting a number
  never observed would be a fabrication, not a correction. Native tool
  duration itself is unaffected and unrelated: it is still measured
  independently, from `ToolStarted` to the engine's own `ToolResult`.
- **It resets per message, never accumulates, and cannot be summed into a
  total-turn elapsed.** A turn with three assistant messages reports three
  independent intervals, not a running total - so reading `duration_ms`
  across a turn's events answers "how long did each step take", not "how
  long has this turn run so far". Summing every `InvocationCompleted` this
  turn is not that total either: each interval already includes its own
  tool time (above), so adding tool time again double-counts it; the sum
  also excludes whatever ran before the first attempt's `StreamState::new`
  mark, any tail still in flight when the turn ended without a closing
  `InvocationCompleted`, and the gap a retry spends outside `drive` between
  attempts. There is no field here that reports complete total-turn elapsed
  time, and none should be inferred from these intervals.
- **Retries, errors and interruption:** a turn that fails outright before any
  `Assistant` arrived records an `InvocationFailed` instead, whose `EventKind`
  carries no `duration_ms` field at all - there is no completed span to
  report. The persisted `Invocation` for that failure still needs a
  `started_at`/`finished_at` pair (both fields are required), so it uses the
  moment the failure was found for both, deliberately the same instant
  rather than a measured span; this is unchanged by this fix and out of its
  scope, since there is no `duration_ms` here to be wrong. A background task
  or a parked wait that stops the process before the next `Assistant`
  behaves the same way: no invocation duration is fabricated for work that
  never produced one.

Implementation note for `turn_engine.rs`: the boundary is tracked with
`tokio::time::Instant`, a monotonic clock, rather than by diffing two
wall-clock (`chrono`) reads - a system clock adjustment between two `now()`
calls must never turn into a negative or wildly wrong duration. The
persisted `Invocation`'s own `started_at` is then derived by subtracting that
same measured span from `finished_at`, so the event's `duration_ms` and the
invocation's own `duration_ms()` never disagree.

## A third-party engine, end to end

`crates/aether-engine/examples/line_engine.rs` is a complete engine in ~200
lines, for a CLI that speaks newline-delimited JSON on stdout and reads
answers on stdin. It spawns the process, maps each line to normalized events,
routes permission prompts through `EngineRun::decisions`, kills the child on
cancellation, and always ends with `Done`. Nothing about it is special-cased
inside Aether.

Run the demo against the scripted CLI beside it:

```sh
cargo run -p aether-engine --example line_engine -- \
    crates/aether-engine/examples/fake_line_cli.sh
```

`crates/aether-engine/tests/third_party_engine.rs` drives that same engine
through the trait and holds it to the contract above: the session first, live
deltas, a permission prompt answered both ways, the tool result the engine
reports, usage on the outcome, and `Done` last even when the turn is
cancelled. `cargo test -p aether-engine` runs it.

## Registering one

Implementing the trait is the whole extension point for the engine itself.
Making Aether *route* to it is still two in-tree edits, and deliberately so —
the model registry is the single source of truth for what a `provider/model`
means:

1. Add a `Protocol` variant in `crates/aether-model/src/registry.rs` (and to
   `Protocol::ALL`), then a `ProviderDef` in `crates/aether-model/src/builtin.rs`
   with its models, its `AuthMethods::cli(...)` login and its `family`.
2. Map that protocol to your `EngineKind` in
   `crates/aether-runtime/src/turn_engine.rs`, and detect the CLI in
   `crates/aether-runtime/src/lib.rs`.

Once registered, an agent on `your-cli/your-model` gets Aether's tasks,
delegation, approvals, events and cost with no further work.

## Version and capability preflight

Before an adapter is registered, its `detect_verbose()` (`crates/aether-engine/src/detect.rs`)
runs the CLI and reports *why* it is unusable rather than collapsing every
failure to absent. `EngineVersion` parses `major.minor.patch` out of
`--version` output by scanning for a dotted-numeric token instead of assuming
a fixed position, since the two CLIs disagree on where it sits (`claude
--version` prints it first, `codex --version` last); a trailing
`-prerelease`/`+build` tag on that token is stripped first, so a dev build
between numbered releases parses to its base version rather than looking
unparsable. `DetectError`'s variants - `NotInstalled`, `ExitFailure`,
`UnparsableVersion`, `TooOld`, `MissingCapability`, `TimedOut` - each name the program
and the remedy; `aether doctor`'s engines check and the daemon's startup log
(`tracing::warn!`) both print the value's `Display` verbatim.

Each version or capability probe has a 15-second deadline and no terminal input.
A timeout reports which probe failed; timeout or caller cancellation kills the
direct child instead of leaving the probe running. These are local CLI checks,
not model calls. The bound is per probe, not a deadline for total daemon startup.

A version floor is only ever a registration gate (`DetectError::TooOld`) once
it is independently verified for everything that adapter's `args()` sends on
every turn - not just one turn kind. Claude Code's 2.1.219 floor is verified
for its offline-sandbox network gate alone (`validate_offline_support`), so it
does not gate registration: a `claude` below it still registers and runs
ordinary, network-enabled turns exactly as before this preflight existed;
only a turn that explicitly denies network access fails, with the existing
"Claude Code 2.1.219 or newer" error. `ClaudeCodeEngine::limitation()` carries
that gap as data rather than a refusal, naming the detected version and the
floor; `aether doctor`'s engines check renders it as "usable, with limits",
distinct from an engine that failed to register, and the daemon log warns
with the same wording rather than the "engine CLI unusable" line an actual
refusal gets.

| Adapter | Minimum version | Capability probe | Why |
|---|---|---|---|
| Claude Code (`claude_code.rs`) | 2.1.219 (offline turns only; not a registration gate) | none | `validate_offline_support` is the one path this floor is actually verified for - Claude's native sandbox network gate. `args()`'s other flags (`--replay-user-messages`, `--include-partial-messages`, `--effort`) are not independently verified against an older CLI, so an unverified floor cannot refuse registration for them; a below-floor `claude` still registers, with the gap reported as a limitation rather than `DetectError::TooOld`. |
| Codex app-server (`codex_app.rs`) | none | `codex app-server --help` must exit zero | No numeric floor is defensible for Codex: its `-c key=value` overrides are TOML overlays that silently no-op on an unrecognized key rather than failing, so a version number would not actually catch a lost capability. The structural probe does: a CLI that renamed or dropped the `app-server` subcommand fails this immediately, which is exactly the capability-loss case preflight exists to catch. |
| Codex exec fallback (`codex.rs`) | none | none | Only reached when `codex_app`'s probe fails (older CLI, no app-server). Same reasoning as above for why no version floor is enforced; this adapter's own `args()` uses no flag known to be version-gated. |

`aether-runtime`'s `detect_engines` tries `CodexAppEngine` first and falls
back to `CodexEngine` only on its failure, logging both reasons together when
neither works (collapsed to one when they are identical, the common
CLI-simply-absent case). `aether doctor`'s engines check reports the same
per-adapter reason instead of a bare "not found", including the
legacy-fallback reason for Codex; a turn dispatched onto an engine detection
ruled unusable fails with that same reason rather than a generic
"not installed" message. Detection never makes the daemon refuse to start: a
CLI that fails preflight is omitted from the engine table, exactly as an
absent one always was, and the API providers remain unaffected. `DetectError`
keeps `TooOld` for a future adapter whose floor genuinely is verified across
everything its `args()` depends on; Claude Code's own floor does not use it.

## Context and native prompts

Default context assembly has no shared coding or delegation policy. Authored
model-family guidance is opt-in. The engine context
is the snapshot's concatenated textual layers, including role, task, model,
checkpoint and retrieved memory. On CLI engines, changing runtime status is
summarized in the new user input: counts and at most eight child IDs,
prioritizing failed or blocked children. Scope, task instructions, checkpoints,
and operator-authored task guidance travel in the user input. Snapshots
record that placement and the exact input. Tool schemas are
supplied by the provider/MCP protocol, not duplicated in another prompt.

Retrieved memory is ranked on the task objective, not on each message, so the
set stays stable across a session. Both engines receive it in the user turn,
digest-gated like the task and checkpoint, so an unchanged set is not re-sent.

Ordinary runtime turns leave the native base instructions untouched. Explicit
extra adapter instructions use Claude's `--append-system-prompt` or Codex's
developer instructions. Isolated summary invocations can still replace the base
instructions. Old sessions may retain previously injected instructions; native
settings only take effect when the relevant session/configuration is refreshed.
See [current limitations](known-limitations.md); the retired migration checklist
is retained in the historical archive.

Aether exposes `inspect_state`, `list_models`, and `wait_for(wake_at=...)` to
engines as well as API models. Absolute wake times include a timezone, must be
within one year, and use the existing durable wait scheduler. Inbox messages
can wake an agent earlier. Recurring interval/cron jobs use `create_schedule`,
`list_schedules` and `update_schedule`. See [the coordination tool inventory](harness-tools.md)
for the current surface and remaining state-retrieval gap.

## Images on a message

A person can attach images to a message (see [the TUI guide](tui.md#images)
and `aether send --image`). The daemon stores each one as a file and the
message carries a reference; the bytes are read back only to hand a model or
an engine the picture.

One rule holds for every engine and model: **the text of the user turn always
names the stored file**, one line per image after the message's own text:

```text
[image: shot.png, 47.1 KB, stored at /home/user/.local/share/aether/aether.attachments/<sha256>.png]
```

So an image is never lost silently. Where the model can see images it gets
the picture as well; where it cannot, the agent still has a path it can open
with its own tools, and the turn does not fail.

| Runs on | What it receives besides the text |
| --- | --- |
| Claude Code | `image` content blocks (base64) after the text block of the stream-json user line, on the first message and on a message steered into a running turn |
| Codex app server | a `localImage` input item per image, naming the stored file, on turn start and on a steer |
| Codex exec | `--image <file>` per image, on a fresh run and on `resume` |
| API model with `vision = true` | `ContentBlock::Image`, which each adapter writes in its provider's shape |
| API model without vision | a text line in place of the image: `[image: shot.png, 47.1 KB] (this model takes no images; open the stored file named above)` |

A message without images is byte for byte what it was before: the same
stream-json line, the same arguments, the same request body.

Messages that wait in an agent's inbox together are folded into one user turn
only while their images stay within one message's limits (10 images, 10 MiB).
The rest stay unconsumed: a running engine is steered with them as soon as it
takes input, and otherwise they open the next turn. An input line therefore
never carries more than one message may, which keeps Claude Code's echo of it
under the 16 MiB line the reader accepts.

An API model is sent its whole history with every request, so the images of
earlier messages are bounded across the request, not only per message: the
newest 20 images and 15 MiB of them go as images, and every older one becomes
a text line, `[image: shot.png, 47.1 KB] (an earlier image, left out to keep
this request in its limits; open the stored file named above)`. The newest
message always fits whole, and the transcript keeps every reference, so
nothing is forgotten: the request stays inside the 32 MB a provider accepts,
however many screenshots a long conversation collects. Images a tool returned
count against the same budget. Claude Code and Codex keep their own history
and are handed only the new message's images.

The attachment directory is always an absolute path, also when
`AETHER_DATA_DIR` is relative: an engine opens the file from its agent's
workspace, not from the daemon's working directory.

If a stored file has been removed, the engine or model gets the text only and
the daemon logs a warning; a resumed or compacted agent never fails on an old
message. Stored references survive a daemon restart, and a turn retried after
one reads the same files again.

A third-party engine reads `EngineTurn::images` for the turn's message and
`EngineInput::images` for a steered one. Each `EngineImage` is a `path` and a
`media_type`. An engine that ignores them loses nothing: the prompt already
names every file.

Images go from a person to an agent. Agent-to-agent `send_message`, agent
replies and tool results stay text.

## Provider exhaustion

When an engine or model reports its quota or usage window exhausted on every
fallback, Aether suspends the agent instead of failing it: the fleet, TUI and
`aether status`/`agents` show it as blocked and exhausted, and the scheduler
re-dispatches the interrupted turn on a backoff schedule once the window is
expected to have cleared. `aether resume <agent>` forces an immediate probe
without waiting for the schedule.

## Transient provider and network failures

A native turn that fails with a network error (`EAI_AGAIN`, `ECONNREFUSED`,
"Can't reach the API server", Codex's "workspace routing discovery failed"
and similar), a timeout, provider capacity, or a 5xx-style server error does
not fail the agent. The turn records the failure, waits, and tries again:
15s, 30s, 1m, 2m, 4m, then every 5 minutes. After a network failure it first
checks that the provider host resolves and accepts a TCP connection, so no
engine launch is spent while the host is offline. Behind an `HTTPS_PROXY` or
`ALL_PROXY` that check is skipped, because a direct connect proves nothing.

How the next attempt resumes depends on how far the failed one got:

- **Nothing reached the model**, proven by the adapter (Codex failed before
  writing `turn/start`, or could not reopen its thread) or by the provider
  refusing the prompt for capacity: the same prompt is sent again on the same
  session. This includes an agent's first turn, when the engine could not
  start at all, and a Codex thread that could not be reopened for a transient
  reason: that thread keeps its context and is resumed, never replaced by a
  fresh one.
- **The model had already answered or run tools**: the engine resumes the
  same session (`--resume`, or the Codex thread) with a short note that the
  turn was interrupted and should continue where it left off. The prompt is
  never sent again, and never into a fresh session. If the engine reported no
  session to resume, the agent fails, as it did before.
- **The prompt may have been submitted but nothing came back** (a network
  or server error, or a start timeout, with no output, tool call or usage,
  after the engine opened its session or Codex wrote `turn/start`): the
  session may or may not hold it, so it is resumed with the prompt and a note
  to check the conversation first and continue rather than repeat work
  already started. Without a known session the agent fails.

A retry starts only once the failed process has exited. Its late receipts
still count: a usage or quota limit reported while it shuts down takes the
suspension path instead, and its closing usage is billed before the wait. A
grant that receipt crossed stops the agent as a budget stop, with no further
launch, even when the retry window ran out at the same time. If the process does not exit within the cleanup grace, its session is
not resumed and the agent fails, saying so.

Each wait emits an `engine.transient_retry` event with the engine, error
class, `attempt`, `delay_ms`, `next_at`, `waiting_for` (`network` or
`provider`) and `resume` (`replay`, `continue` or `recheck`). The TUI shows it
as a notice in the agent's transcript. Cancelling the agent interrupts the wait at
once. Budget, deadline, quota and usage-limit handling is unchanged:
exhaustion and context limits outrank every transient signal and take the
fallback or suspension path above.

Retries stop after a wall-clock window,
`[scheduler] engine_transient_retry_minutes` (two hours by default; `0` fails
on the first error). The window is a hard bound per turn: it starts at the
turn's first transient failure, partial progress does not reset it, and no
attempt starts after it ends. The agent then fails, the error names the class
and how long it retried, and every responsible ancestor is told. The turn
keeps its scheduler and engine-cap slot while it waits.
