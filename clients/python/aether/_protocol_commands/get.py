# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from ..protocol import (
    AgentId,
    Any,
    ArtifactId,
    CheckpointId,
    EventQuery,
    InvocationId,
    NotificationId,
    RunId,
    ScheduleId,
    SnapshotId,
    TaskId,
)

def get_agent(*, id: AgentId) -> dict[str, Any]:
    """The `get_agent` command."""
    frame: dict[str, Any] = {"cmd": "get_agent"}
    frame["id"] = id
    return frame

def get_artifact(*, id: ArtifactId) -> dict[str, Any]:
    """The `get_artifact` command."""
    frame: dict[str, Any] = {"cmd": "get_artifact"}
    frame["id"] = id
    return frame

def get_attention() -> dict[str, Any]:
    """Everything waiting on the operator: pending approvals, unanswered
    questions, and failed or blocked agents, read at one sequence number.
    """
    frame: dict[str, Any] = {"cmd": "get_attention"}
    return frame

def get_checkpoint(*, id: CheckpointId) -> dict[str, Any]:
    """The `get_checkpoint` command."""
    frame: dict[str, Any] = {"cmd": "get_checkpoint"}
    frame["id"] = id
    return frame

def get_events(*, after: int, limit: int, query: EventQuery) -> dict[str, Any]:
    """The `get_events` command."""
    frame: dict[str, Any] = {"cmd": "get_events"}
    frame["after"] = after
    frame["limit"] = limit
    frame["query"] = query
    return frame

def get_git_summary(*, workspace: str) -> dict[str, Any]:
    """Branch and dirtiness of a workspace on the daemon's machine."""
    frame: dict[str, Any] = {"cmd": "get_git_summary"}
    frame["workspace"] = workspace
    return frame

def get_github_watcher() -> dict[str, Any]:
    """Inspect the opt-in GitHub watcher and its per-repository health."""
    frame: dict[str, Any] = {"cmd": "get_github_watcher"}
    return frame

def get_inbox(*, after: int, agent: AgentId) -> dict[str, Any]:
    """The `get_inbox` command."""
    frame: dict[str, Any] = {"cmd": "get_inbox"}
    frame["after"] = after
    frame["agent"] = agent
    return frame

def get_invocation(*, id: InvocationId) -> dict[str, Any]:
    """The `get_invocation` command."""
    frame: dict[str, Any] = {"cmd": "get_invocation"}
    frame["id"] = id
    return frame

def get_notification(*, id: NotificationId) -> dict[str, Any]:
    """One notification. Needs `features::NOTIFICATIONS`."""
    frame: dict[str, Any] = {"cmd": "get_notification"}
    frame["id"] = id
    return frame

def get_pairing(*, id: str) -> dict[str, Any]:
    """Read a pairing's state; the code is never repeated. Host only."""
    frame: dict[str, Any] = {"cmd": "get_pairing"}
    frame["id"] = id
    return frame

def get_push_config() -> dict[str, Any]:
    """Whether the daemon sends Web Push, and the key to subscribe with."""
    frame: dict[str, Any] = {"cmd": "get_push_config"}
    return frame

def get_remote() -> dict[str, Any]:
    """The remote listener's settings and state. Host only."""
    frame: dict[str, Any] = {"cmd": "get_remote"}
    return frame

def get_run_report(*, run: RunId) -> dict[str, Any]:
    """Compose a run's report from its events."""
    frame: dict[str, Any] = {"cmd": "get_run_report"}
    frame["run"] = run
    return frame

def get_schedule(*, id: ScheduleId) -> dict[str, Any]:
    """The `get_schedule` command."""
    frame: dict[str, Any] = {"cmd": "get_schedule"}
    frame["id"] = id
    return frame

def get_snapshot(*, id: SnapshotId) -> dict[str, Any]:
    """The `get_snapshot` command."""
    frame: dict[str, Any] = {"cmd": "get_snapshot"}
    frame["id"] = id
    return frame

def get_task(*, id: TaskId) -> dict[str, Any]:
    """The `get_task` command."""
    frame: dict[str, Any] = {"cmd": "get_task"}
    frame["id"] = id
    return frame

def get_transcript(*, after: int, agent: AgentId) -> dict[str, Any]:
    """The `get_transcript` command."""
    frame: dict[str, Any] = {"cmd": "get_transcript"}
    frame["after"] = after
    frame["agent"] = agent
    return frame

def get_transcript_page(*, agent: AgentId, after: int | None = None, before: int | None = None, limit: int | None = None) -> dict[str, Any]:
    """A bounded page of an agent's transcript: entries after `after`, the
    newest before `before`, or the newest of all when neither is given.
    `limit` is 1 to 1000, 200 when absent. Needs `features::TRANSCRIPT_PAGING`.
    """
    frame: dict[str, Any] = {"cmd": "get_transcript_page"}
    if after is not None:
        frame["after"] = after
    frame["agent"] = agent
    if before is not None:
        frame["before"] = before
    if limit is not None:
        frame["limit"] = limit
    return frame

def get_tree(*, after: AgentId | None = None, include_archived: bool | None = None, limit: int | None = None, role_prefix: str | None = None, root: AgentId | None = None, run: RunId | None = None) -> dict[str, Any]:
    """The live graph: the agents in the run (or all runs) with relations.
    One page: the newest live agents, then the newest agents of any state,
    at most `limit` of each (1 to 500, 500 when absent); `AgentTree.next_after`
    says older ones were left out.
    """
    frame: dict[str, Any] = {"cmd": "get_tree"}
    if after is not None:
        frame["after"] = after
    if include_archived is not None:
        frame["include_archived"] = include_archived
    if limit is not None:
        frame["limit"] = limit
    if role_prefix is not None:
        frame["role_prefix"] = role_prefix
    if root is not None:
        frame["root"] = root
    if run is not None:
        frame["run"] = run
    return frame
