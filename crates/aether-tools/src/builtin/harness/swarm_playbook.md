# Aether swarm playbook

A swarm is one lead and 2-8 workers building one change in parallel. Use it
when the change splits into three or more parts that own disjoint files. For
less, do the work yourself or delegate one child.

## Lead

1. Research before anyone builds. Read what the change touches and its
   callers, then write the plan. Each task has: an id; an objective; the paths
   it owns, disjoint from every other task's; the interfaces it provides and
   consumes, with exact signatures; acceptance commands; the task ids it
   depends on.
2. Give each worker its own task, the shared interfaces and the Worker rules
   below, nothing more: not your conversation, not the other tasks' details.
   Workers start cold and cannot read this playbook.
3. Delegate one worker per task: `delegate` with `leaf: true`,
   `workspace_mode: "isolated"`, `allowed_paths` set to the task's owned
   paths, `wait: false`, a fast-tier model from `list_models` and an explicit
   effort. Start a task only after the tasks it depends on, or tell it which
   peer to wait for.
4. Wait with `wait_for(tasks=[...], any=true)` and read each result as it
   lands.
5. Integrate in plan order with `integrate_workspace(child_agent_id)`. A
   refusal lists every conflicting path: resolve them in your workspace, or
   hand the report to a new worker, then integrate again. Run the task's
   acceptance commands after each integration and the project's fast checks
   at the end.
6. Re-dispatch a failed task to a new worker with the failure evidence, at
   most twice. Then do it yourself, or fail with the evidence.

## Worker

- You own your task's paths only; writes elsewhere are refused. Keep the
  interfaces you provide exactly as planned. If one must change, message its
  consumers before you change it.
- `list_peers` shows the other workers your lead delegated: id, name, role,
  status and task objective.
- `send_message(to=<peer id>, content=...)` tells a consumer that an
  interface is ready, or asks a provider one precise question. Name the
  path, the signature and its state.
- To block on a peer, `wait_for(message_from=<peer id>, timeout_seconds=...)`.
  Bound every wait, and never wait on a peer that waits on you.
- Messages reach your parent's subtree (your peers, their children, your
  lead) and your own ancestors, nothing else.
- Run your acceptance commands, then `complete_task` with the changed paths,
  the commands and their results, and any note about an interface. No commit
  is needed: integration copies your worktree's files.

Without `list_peers` (older Aether), workers run without peers and the lead
relays every message.
