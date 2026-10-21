# Daemon state and panic audit

Scope: daemon startup, recovery, scheduler, connections and their store, runtime,
model, engine, MCP, tool, remote, context and memory dependencies. Test-only
items are excluded. The search covers `assert!`, `assert_eq!`, `.unwrap()`,
`.expect()`, `unreachable!` and `panic!`; debug assertions at touched storage
boundaries were checked too. Locations below use module/function names so they
remain useful after line numbers change.

## Stored rows, event history and artifacts

- `aether-store/sqlite/notifications::put`: an empty title returns a
  corruption error with `aether doctor`.
- `sqlite/repository_aliases::{preview_alias,register_alias}`: invalid alias
  input returns corruption errors; self-referential stored aliases also fail
  validation rather than asserting.
- `sqlite/push_subscriptions::{put,remove,remove_if}`: row-count assertions are
  errors naming the operation and `aether doctor`.
- `sqlite/{memories,memory_feedback,mutations}`: stored ranking, source and
  revision assumptions are validated before mutation instead of debug asserts.
- `aether-core/idempotency/ledger::{start,next,forget}`: assertions remain core
  programmer contracts. `aether-store/sqlite/ledger` validates persisted store,
  epoch and instance identities, ordered watermarks and ordinal exhaustion
  before calling them. A damaged receipt ledger refuses the affected operation.
- `aether-daemon/push::catch_up`: both event-page bounds/progress assertions
  become a replay error; the notification stream remains isolated.
- `aether-daemon/push/StoreFleet::agent`: mismatched stored identity returns an
  error. Shared agent/task/lease decoding also rejects mismatched row keys.
- `aether-daemon/connection/dispatch::read_artifact_slice`: an oversized result
  returns a store corruption error rather than asserting.
- `aether-runtime/artifacts::evict`: an already-evicted artifact returns an
  actionable validation error.
- `aether-core/agent::format_usd`: non-finite persisted/provider cost is rendered
  as unavailable with `aether doctor`; the nonempty locally formatted result
  assertion remains.

Agent, task and lease lists decode each row separately, preserve corrupt bytes,
and record persistent quarantine notices. Single reads reject corruption.
Event histories deliberately remain strict: silently dropping an acquisition or
cancellation could let a surviving process overlap new work. Indexed event
sequence, identity and owner must match the decoded envelope. Recovery parks
only the affected agent where its ownership can be reconstructed. An unreadable
global cancellation journal withholds dispatch while diagnostics remain served.

Workspace, build-cache, scratch and engine-session collection refuses deletion
when ownership reads discover quarantined agent, lease or task rows. A partial
list cannot prove that a directory is unused; repair the reported records and
retry `aether storage gc`. Other agents can continue serving during this refusal.

## Checkpoints, workspaces and recovery

There were no production panic macros in checkpoint deserialization, recovery
classification or workspace GC itself. These paths nevertheless propagated a
single bad record through startup. Live references now validate workspace
directories and checkpoint artifact identities. Interrupted recovery has a
per-agent unwind/error boundary and keeps existing pending cancellation and
lease/resource inspection behavior. Original corrupt history is preserved.
Missing references produce an inspection reason and the commands `aether doctor`
and `aether resume AGENT` after repair.

## Configuration, engines, providers and MCP

- `aether-model/config/{storage,retention}`: four `NonZero::new(...).unwrap()`
  expressions are constant expressions on positive literals; retained.
- `aether-model/adapters/{openai_chat,openai_responses}/request`: `unreachable!`
  checks the object made by the immediately preceding `json!`; retained.
- `aether-engine::{kill_group,orphans::kill_matching_group}` and
  `codex_app/{loopback,read_usage/process}`: PID range assertions guard spawned
  child handles or a PID explicitly range-checked before the kill helper.
  They do not trust provider text as a process-group ID; retained.
- `codex_app/read_usage`: timeout/deadline and RPC ID/method assertions operate
  on fixed internal deadlines and locally constructed requests; retained.
- `aether-tools/capture`: the capture limit is an internal positive constant.
  `builtin/web` and `aether-core/redact` regex `expect`s compile literal patterns.
- `aether-core/{shell_network,agent/config}` assertions check the result of
  local command classification, monotonic strengthening and consumed tokens.

Provider response and MCP payload decoding uses fallible deserialization; no
unchecked production unwrap/expect was found on those payloads. Truncated TOML
remains a startup refusal with the file, offending setting and repair command.
Security/listener configuration is never silently replaced with defaults.

## Remote and socket input

- `aether-remote/push/delivery`: a recipient/subscription device mismatch now
  returns an error; the locally bounded in-flight queue assertion remains.
- `aether-remote/push/vapid::authorization`: both audience assertions are HTTPS
  origin validation errors. P-256 public-point length/tag and nonempty freshly
  encoded key bytes remain assertions on local cryptographic output.
- `push/notice::{compose,test_notice}` instance identity assertions are reached
  only after stored identity validation. Topic length, truncation room and
  nonempty local notice family remain internal construction contracts.
- `push/subscription::audience` HTTPS/port assertions follow endpoint validation;
  `push/transport` body/topic assertions follow local encryption/notice creation.
- `push/ece` salt size, derived key material, record delimiter and encrypted body
  bounds assert local cryptographic construction. `push/notifier` capacity,
  deadline and one-time device-store initialization are internal wiring.
- `push/outbox` retry/jitter, capacity, one-send-per-device and dead-subscription
  `unreachable!` checks operate on local queue transitions.
- `aether-remote/{lobby,listener/write_deadline,ws/meter}` assertions enforce
  internal capacity, positive deadlines, byte-meter bounds and sender ownership.
  Daemon configuration validates limits before constructing these services.
- `aether-daemon/connection/writer::encode` asserts the protocol window already
  admitted by the handshake. Command-family `unreachable!` branches in
  `dispatch/{agents,control,memory,notifications,reads,runs,schedules,system,tasks}`
  and runtime `turn/actions/{delegation,inspection,lifecycle,schedules}` check
  the internal exhaustive router; unknown wire commands fail deserialization.
- `connection` subscription insert/prune assertions guard locally generated
  subscription IDs and entries just selected from that same map.
- `remote/admin::revoke` asserts the revocation timestamp just assigned locally.
- `connection/host_services/{attention,complete,workspaces,git/work_tree}` checks
  bounded local collections, unique validated agent IDs, canonicalized absolute
  paths, positive validated limits and already checked git object lengths.

Other retained programmer contracts: `runtime/exhaustion` checks its own JSON
object; `runtime/storage` receives a positive validated backup cap;
`commands/termination` matches a status checked immediately above;
`store/history_retention` checks a locally computed non-deleting guard;
`SqliteStore::with_submission_limits` is test wiring; core attachment size and
protocol compatibility assertions are compile-time constants; core idempotency
submission recording asserts the internal admitted-write/replay distinction.

## Limits

This does not repair arbitrary SQLite page damage or invent missing security
configuration. A store that cannot be opened/migrated, or invalid listener/auth
configuration, can still refuse startup with diagnostics. A panic boundary cannot
recover process abort, SIGKILL, kernel failure or allocation abort. Assertions on
true internal invariants remain useful bug detectors; task supervision isolates
unwinds rather than treating partially completed work as successful.

## Background-task supervision

The GitHub watcher and scheduler restart after an unwind/unexpected exit with
1–60 second exponential backoff. Shutdown interrupts backoff; a running worker
gets five seconds to observe cancellation before its owned future is dropped.
No replacement worker overlaps the failed future. Tasks whose queue/stream state
cannot be reconstructed safely (MCP discovery/health recorder, push follower and
delivery, transport control/listener, retention and disk guard) are isolated and
publish a disabled notice instead. Periodic maintenance's existing blocking jobs
continue retrying failures at their normal interval. Lease heartbeats publish the
affected agent ID on failure. Agent turns already contain their own unwind
boundary and release their running slot through a drop guard.

Budget/deadline failures with a known subtree and task dependency failures with a
known owner withhold that scope only; their checks retry rather than caching an
unverified result. Failure to enumerate the scope still withholds the pass.
Corrupt descendants cannot disappear from cancellation completion: the final
check follows stored subtree IDs and strictly reads every member before clearing
the durable intent. Per-agent scheduler wake failures are isolated as well.
