# Aether coordination tools

An agent created with `leaf: true` cannot delegate children. Aether omits
delegation, fleet, task-listing, model-listing, and schedule tools from both
native and API model tool lists, and `swarm_playbook`, which is for the lead. A
delegated leaf keeps `send_message`, `inspect_state` and `list_peers`, so
workers can coordinate with each other. `leaf` is set at creation and cannot be
changed on a live agent. A root agent has no peers, so it is not offered
`list_peers`.

API agents receive these tools through the model tool catalog. CLI engines
receive them through MCP server `aether`; deferred catalogs use the engine's
native tool discovery. The schemas in `aether-tools/src/builtin/harness.rs`
`inspection.rs` and `schedules.rs` define the accepted arguments.

| Workflow | Tools | Behavior |
|---|---|---|
| Discover models | `list_models` | Configured provider/model IDs, supported effort, availability and a `tier` (`fast`, `balanced`, `strong`, `frontier`, or null) per model. Pick by tier, not name; see the model tiers note in [configuration](configuration.md). |
| Inspect current state | `inspect_state` | Own agent/task, model, effort, context, budgets, concurrency and compaction policy. |
| Retrieve stored output | `read_artifact` | Scoped UTF-8 text pages with byte offsets and continuation cursors; available to API agents and CLI engines. |
| Inspect delegated work | `list_agents`, `list_tasks`, `read_task` | Scoped metadata pages and bounded task/result reads, including completed descendants. |
| Delegate | `delegate` | Creates a tracked child and task; requires explicit `model` (`provider/model`) and `effort`; neither is inherited. Returned agent and task IDs are different identifiers. `workspace_mode` selects a shared workspace (the default) or an isolated Git worktree. `host_context` (`full` or `lean`) is inherited from the parent when omitted; see [lean host context](engines.md#lean-host-context). |
| Integrate a child workspace | `integrate_workspace` | Applies an isolated child's changes to its parent's workspace after the child finishes, with conflict checks. The child workspace is retained. |
| Steer | `send_message` | Messages an agent; `subtree=true` also delivers to its live descendants. A delegated agent reaches its parent's subtree and its own ancestors; a `subtree=true` message only its parent's subtree. Root agents are not limited. |
| Coordinate with peers | `list_peers` | The other agents the caller's parent delegated: id, name, role, status, task and a short objective. Offered to delegated agents only. |
| Run a swarm | `swarm_playbook` | How a lead plans disjoint tasks, delegates leaf workers in isolated workspaces, how peers coordinate, and how the lead integrates in plan order. Offered to agents that may delegate, not to leaves. |
| Stop work | `cancel_agent` | Cancels an agent and its live descendants through runtime authority checks. |
| Return a result | `complete_task`, `fail_task` | Settles the assigned task with a summary or error and optional structured data. |
| Wait or ask | `wait_for`, `ask_user` | Durable waits for tasks/messages/time, or a question for the person. |
| Preserve knowledge | `remember`, `request_checkpoint` | Stores reusable knowledge or requests validated context compaction. |
| Repeat work | `create_schedule`, `list_schedules`, `update_schedule` | Durable interval/cron triggers, delivery history, pause/resume/cancel. |

The protocol `create_agent` command also requires explicit `config.effort` whenever
`parent` is set: `low`, `medium`, `high`, `xhigh` or `max`. Missing effort is refused
before creating an agent or task. Roots may omit effort and use the model default;
restart and wake preserve the existing agent’s recorded effort.

`wait_for(wake_at=...)` schedules one timezone-qualified wake within one year;
messages may wake the agent sooner. Recurrence uses `create_schedule` with
`every_seconds` or a five-field `cron` expression and optional IANA `timezone`.
Use `max_deliveries` to bound recurrence. A busy target skips the occurrence
by default; `busy="queue_one"` delivers anyway but never stacks a second unread
trigger. Trigger delivery does not imply task completion; inspect results and
delivery history separately. Timers that start a fresh run per occurrence are
operator-only (`aether schedule create --objective`); agents see them in
`list_schedules` only when they target them, which spawn timers never do.

`remember` takes a `scope`: `repository` (the default; the agent's own scope
when it has no repository), `global`, `role` (agents with the caller's role,
in any repository), `agent` (only the caller), or `subtree` (the caller and
every agent beneath it, up to eight generations down). A subtree memory is
always rooted at the caller. Repository notes committed under
`.aether/knowledge/<role>/` reach only agents of that role family in the
repository: the role named by the directory and every role that extends it at
a dot, so a note in `team/` reaches `team.lead`. Role directories are one
level deep. They filter relevance; they are not access control.

Native file, command and web capabilities remain available through the CLI
engine. Aether owns coordination; a native subagent is not a tracked Aether
child. Tool exposure and scope settings still apply to each agent.

### Delegated workspaces

`delegate` defaults to `workspace_mode="shared"`, preserving the existing
workspace and delegation behavior. Set `workspace_mode="isolated"` to give the
child a separate Git worktree. The optional `workspace_root` names the source
Git repository; when omitted, Aether uses the parent's workspace root. The
source must be a Git top-level directory within the parent's scope. An
isolated child can delegate again, choosing a shared or isolated workspace for
each of its own children. Isolated delegation currently requires the host
execution backend; the container backend cannot access a linked worktree's Git
metadata outside its workspace mount.

An isolated child sees a snapshot of the source workspace at delegation time.
The snapshot includes tracked working-tree content (including staged,
unstaged, and deleted paths) and nonignored untracked files. Ignored files are
excluded. Sparse checkouts, unresolved merges, and submodules are refused
because a working-tree snapshot of them would be incomplete or ambiguous.
Snapshot content is written to the local repository's Git object database and
kept reachable by a managed ref, including any nonignored untracked files.
Its worktree and task result remain available when the child finishes or fails.
Completion wakes a waiting parent independently of integration: the parent
can inspect the result and changes before calling
`integrate_workspace({"child_agent_id":"agt_..."})`. Integration is an explicit
parent action and refuses conflicts with edits made in the parent since the
snapshot. It copies resulting file changes, including committed changes in the
child; it does not transplant the child's Git commits or branch history. It
copies only changes present in the child's own worktree, so the child may
leave a completed isolated grandchild's result unintegrated. That grandchild's
worktree remains available for later inspection or integration.

Integration does not reset or check out over the parent's working tree. The
worktree is retained after integration for inspection and must be removed
deliberately when no longer needed. An operator can do the same from the
command line with `aether workspace diff|integrate|discard AGENT` (see
[workspace review](protocol.md#workspace-review)); the operator's integration
runs the same conflict check as the tool. Replaced regular files keep their read and
write permissions, while Git's executable setting controls execution;
setuid, setgid, and sticky bits are cleared. New integrated files are
owner-only (`0600`, or `0700` when executable). Change permissions explicitly
if others need access. Aether serializes its own integrations and checks the
parent's paths before applying changes. Each file replacement is atomic, but
the set of changes is not. An interrupted integration can leave some files
applied, and a retry rechecks the paths using its retained journal. Concurrent
native tools or other processes do not honor Aether's integration lock, so
keep the destination workspace free of other writes while integration applies.

After preserving the result and making the child worktree clean, use
`git -C <source> worktree remove -- <child-worktree>` to remove it; Git refuses
dirty worktrees without `--force`. The matching private metadata directory and
`refs/aether/workspaces/<id>` (and `refs/aether/integrations/<id>` after an
integration) also remain until explicitly removed. Keep them while you may
need to inspect or retry integration.

Worktrees prevent ordinary concurrent edits from landing in the same working
directory. They are not a filesystem sandbox: native CLI tools can access
paths outside their worktree when their permissions allow it. Keep using
approval and execution policies appropriate for the repository. Worktrees do
not isolate external resources such as ports, databases, or services.

### Waiting for native background work

When a CLI has reported live background jobs, `wait_for` keeps that session
and its jobs alive. The MCP call remains pending until a native job finishes,
an inbox message arrives, the requested task condition is satisfied, or its
deadline expires. This is event-driven: no log polling or model check-ins are
needed. A native completion can wake it before the requested task condition.
The deadline ends the wait, not the background job; use native stop tools or
explicit agent cancellation to stop work.

This attached wait keeps the agent's engine turn/concurrency slot occupied;
it is not a separately parked, restart-durable native process. Cancelling the
agent or shutting down the daemon still stops its engine and attached jobs.
Do not use detached `nohup`/`setsid` processes to evade that ownership. Without
native jobs, the usual durable Aether wait and bounded engine parking apply.
This preservation applies to `wait_for`, not other turn-ending actions such
as `ask_user` or `delegate(wait=true)`. When native work must stay attached,
delegate without an automatic wait, then use `wait_for`.

An interrupted `mcp__aether__wait_for` is safe to retry after restart. Mutating
native calls require inspection on the same host boot; a confirmed Linux host
reboot instead wakes the agent to re-check partial effects before retrying and
releases quarantine keys. See [restart recovery](../ARCHITECTURE.md#safety-accounting-and-storage).

## Bounded state retrieval

`inspect_state` describes the current agent. Use `list_agents` and `list_tasks`
for delegated work (agent rows include model and configured effort): each defaults to ten entries and allows at most 25. Pass
`next_after` as `after` for the next page. Lists reflect current state and include
settled work; agents outside the caller's delegated subtree are excluded.

`list_peers` pages the same way. Its rows are the caller's siblings, the caller
excluded; a message from one wakes a sibling waiting on its inbox or on
`wait_for(message_from=...)` for that sender.

`read_task` returns a character page of the serialized task record, including
its result and structured data. It defaults to 2,000 characters and allows at
most 8,192. For subsequent pages pass both `next_offset` and `revision` from the
response. A changed record produces an error requiring a restart at offset zero;
this prevents concatenating pages from different task versions. Individual
pages need not be complete JSON documents. Completion/failure details also
arrive through the inbox.

This inventory does not prove ordinary-language reliability or provider
steering parity; those acceptance checks remain open
([known limitations](known-limitations.md)).

## Retrieving complete output

`read_artifact(id, offset=0, limit=8192)` reads text artifacts produced by the
caller or its delegated descendants. Offsets are zero-based **byte** offsets;
limits are 4–8192 bytes. Pages end on UTF-8 character boundaries. Pass the
returned `next_offset` until it is null. Unknown, unrelated, missing, evicted,
and non-UTF-8 content produce explicit errors. Reading a page seeks to that
range in disk storage; it does not load the entire artifact.

## API shell sessions

Direct API agents receive `bash`, `shell_output`, `shell_processes`,
`shell_stop`, and `write_stdin`. CLI engines retain their native shell and
file tools; these Aether workspace tools are not added to their MCP catalog.

Every shell command writes combined stdout/stderr to a private, randomly named
`.aether-shell-*/output.log` file in its working directory. Tool results include
`output_path`; use `read_file` line windows, `grep`, `tail`, or `dd` through
`bash` to retrieve the relevant part. Logs preserve raw bytes, including terminal
control sequences. They survive process exit, stop, and handle cleanup, and
remain until removed from the workspace. They are workspace files, not subject
to artifact retention. Each private log directory contains a `.gitignore` so
capturing output does not make the repository dirty.

Large foreground results also keep an immutable artifact copy for the inspector,
export, and `read_artifact`. These copies stream to disk without buffering the
whole log. Copies are hashed during streaming and deduplicated against existing
artifacts. Artifact retention applies to those copies.

`shell_output` returns at most 24,000 raw output bytes per page and reports
`pending_bytes` after that page. Keep reading while it is nonzero, even if
`running` is false. Earlier pages remain in the output log. Capture failures
stop the process and are reported explicitly; output is no longer discarded
when it crosses the former 1 MiB buffer limit.

Start an interactive session with:

```json
{"command":"bash --noprofile --norc", "tty":true}
```

`tty:true` implies background execution and returns a `process_id`. Send input
with `write_stdin({"process_id":"...", "chars":"pwd\n"})`, then read the
response with `shell_output`. Control characters can be sent the same way.
The terminal starts at 80 columns by 24 rows. Writes accept at most 8192 UTF-8
bytes and have a five-second deadline; partial writes report how many bytes
were accepted. Use `shell_stop` to end the session. Writes are mutating shell
operations, with normal approval/resource checks; only the owning agent can
write or stop the process. Both host and container backends support terminals.
The shell git guard checks the command that starts a session, not input typed
with `write_stdin`.

## API filesystem tools

`read_file`, `edit_file`, and `write_file` accept regular files only; pipes,
sockets, directories, and devices return a clear error before being opened.
`write_file` still creates missing files and parent directories. Symlinks are
resolved within the agent's permitted workspace and paths before these checks.
`grep` accepts a regular file or a directory, skipping special entries during
recursive search. `list_dir` requires a directory and lists entries without
opening their contents. Native engines keep their own filesystem tools.
