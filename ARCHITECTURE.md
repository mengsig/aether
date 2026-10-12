# Aether architecture

This describes the current implementation, not the retired all-features roadmap.

## Ownership

One binary provides a daemon and client modes. The daemon owns authoritative
agents, tasks, scheduling, inboxes, events, checkpoints, memory, and artifacts.
The TUI is a client: closing it leaves agents running; stopping the daemon
interrupts attached native processes.

State mutations and their events commit atomically through the store. Clients
render projections of this state. A receipt means input was delivered, not
that the model complied or answered correctly.

## Execution paths

Native Claude Code and Codex engines own their model loop, tools, project
instructions, session context, and automatic compaction. Aether connects its
coordination tools over MCP and consumes native event streams. Native
delegation/scheduling are restricted to keep coordinated work inside Aether.
There is no default Aether coding policy or replacement native workspace toolset.

API models use Aether's provider/tool loop and context assembly. Authored model
profiles are opt-in and apply only here. Compatible endpoints do not necessarily
support every optional model capability.

Explicit checkpoints and provider transitions validate a checkpoint before
replacing a native session. Failed checkpointing retains the original session
and history. Aether records its inputs and observed outputs, not every private
detail inside a native harness.

## Delegation and waiting

Roles are labels, not hard-coded tiers. Each child needs an explicit model and
effort. The runtime validates scope, tools, budgets, and lifecycle transitions
before admitting work. Global, provider, engine, and subtree concurrency limits
apply, with special admission rules for operator-addressed work.

Delegation defaults to a shared workspace. An explicit isolated mode creates a
managed Git worktree from a repository within the parent's scope, using the
source's current working state as the child's starting point. Children can
delegate recursively under the same rules. A child's result and worktree are
retained after task settlement. Its parent can explicitly integrate the
completed child's changes only after conflict checks against the starting
snapshot; task completion and wait conditions do not integrate files.

Persisted task, message, and deadline conditions wake agents. Unset timed
check-ins do not cause periodic model calls. Maintenance and external-source
polling still use timers.

When a native session reports background jobs, `wait_for` holds the MCP reply
and session open until a tracked completion, inbox message, task condition, or
deadline. This retains an engine slot and is not restart-durable. Cancellation
still stops attached work. Other turn-ending actions retain ordinary parking
semantics. See [harness tools](docs/harness-tools.md).

## External inputs

CLI, TUI, clients, MCP, webhooks, timers, and GitHub share the daemon's state
model. GitHub can route to a fixed agent or reuse per-thread agents. Authorized
GitHub activity remains external input, not an authenticated operator message,
unless the operator named the requesting login in the watcher's
`operator_users` and the watcher verified that account wrote the text.
Webhooks use signed deliveries. Phones and browsers reach a loopback HTTP
listener that is off until enabled, through `tailscale serve` or another HTTPS
proxy; each is a device with its own key, enrolled by a one-time code the
operator confirms at the host, and the Host and Origin allowlist names the
public address. `aether remote setup` drives that path and reaches the daemon
only through protocol commands, running Tailscale without a shell, and never
changing host configuration silently. See [remote setup](docs/remote-setup.md).
Another machine's daemon is reached with `aether --remote NAME`, which runs
`aether bridge --stdio` over SSH to that user's Unix socket: no listener, token
or pairing, and the standing of a local client. See [remote control](docs/remote.md).
The `[listen]` TCP transport needs a trusted tunnel: Aether does not terminate
TLS.

## Safety, accounting, and storage

Approval policy, scope, and execution backend are different controls. Default
approval mode is permissive. Native tools do not universally pass through
Aether's guards or resource leases. A separate Git worktree avoids ordinary
working-directory collisions but does not confine native tools. The container
backend covers Aether shell execution, not arbitrary native harnesses or every
tool.

Usage comes from provider/engine observations, with explicit unknown states.
Context occupancy is separate from cumulative input/output/cache usage. Dollar
amounts are reported or estimated, not a subscription bill. The global run-cost
delegation guard does not retroactively stop running turns.

SQLite stores authoritative state/history. On restart, read-only calls and
registered retry-safe tools can be retried. Native shell and file writes with
no outcome require inspection when the host stayed up: a tool process may
still be writing, and its resource keys stay quarantined until `aether resume`
or cancellation. Engine-native read tools and Aether MCP names are resolved
alongside the side-effect class recorded in `tool.requested`.

On Linux, recovery persists `/proc/sys/kernel/random/boot_id` in store metadata
only after the recovery pass succeeds. A changed boot id proves local tool
processes died: interrupted mutations and earlier inspection holds become
retryable, their keys are released, and the agent receives the call names/ids
with instructions to re-check partial effects before redoing work. Requests
already sent to external services may still take effect. Operator pauses stay
in force. Legacy stores without a boot id, an unreadable boot id, and other
operating systems retain conservative inspection behavior.

Recovery does not preserve child OS processes or guarantee exact private native
context. Completed-run retention,
artifact-byte retention, backups, and workspace/native-session files have
different lifecycles.

## Source map

| Crate | Responsibility |
| --- | --- |
| `aether-core` | Domain types, transitions, events, scopes |
| `aether-store` | SQLite persistence, transactions, queries |
| `aether-runtime` | Scheduler, turns, delegation, recovery, external events |
| `aether-engine` | Native CLI adapters and control/stream protocols |
| `aether-model` | API providers, authentication, registry, configuration |
| `aether-tools` | Aether tools, shell backends, MCP integration |
| `aether-context` | API context, inbox rendering, checkpoints |
| `aether-memory` | Durable memory and retrieval |
| `aether-net` | Shared network-safety boundaries |
| `aether-protocol` | Versioned daemon commands and replies |
| `aether-daemon` | Connections and daemon lifecycle |
| `aether-remote` | Phone listener, WebSocket carrier, device pairing, embedded web app, Web Push; the daemon serves it through `RemoteHost` |
| `aether-client` | Rust client and integration surfaces |
| `aether-tui` | Terminal projections, input, inspection, themes |
| `aether` | CLI and binary entry point |

Crates live in `crates/`; generated Python/TypeScript clients in `clients/`;
verification tools in `scripts/` and `evals/`.

## Verification

See [CI](docs/ci.md) and [known limits](docs/known-limitations.md). Narrow
lifecycle tests do not establish general accuracy or performance parity.
