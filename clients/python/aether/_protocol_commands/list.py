# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from ..protocol import (
    AgentId,
    AgentQuery,
    Any,
    ArtifactQuery,
    NotificationState,
    TaskQuery,
)

def list_agents(*, query: AgentQuery) -> dict[str, Any]:
    """The `list_agents` command."""
    frame: dict[str, Any] = {"cmd": "list_agents"}
    frame["query"] = query
    return frame

def list_approval_rules(*, include_revoked: bool | None = None) -> dict[str, Any]:
    """Remembered decisions, oldest first."""
    frame: dict[str, Any] = {"cmd": "list_approval_rules"}
    if include_revoked is not None:
        frame["include_revoked"] = include_revoked
    return frame

def list_approvals() -> dict[str, Any]:
    """The `list_approvals` command."""
    frame: dict[str, Any] = {"cmd": "list_approvals"}
    return frame

def list_artifacts(*, query: ArtifactQuery) -> dict[str, Any]:
    """The `list_artifacts` command."""
    frame: dict[str, Any] = {"cmd": "list_artifacts"}
    frame["query"] = query
    return frame

def list_devices(*, include_revoked: bool | None = None) -> dict[str, Any]:
    """Enrolled devices, oldest first. Host only."""
    frame: dict[str, Any] = {"cmd": "list_devices"}
    if include_revoked is not None:
        frame["include_revoked"] = include_revoked
    return frame

def list_invocations(*, agent: AgentId) -> dict[str, Any]:
    """The `list_invocations` command."""
    frame: dict[str, Any] = {"cmd": "list_invocations"}
    frame["agent"] = agent
    return frame

def list_models() -> dict[str, Any]:
    """The `list_models` command."""
    frame: dict[str, Any] = {"cmd": "list_models"}
    return frame

def list_notifications(*, limit: int | None = None, source: str | None = None, states: list[NotificationState] | None = None) -> dict[str, Any]:
    """Notifications newest first. Needs `features::NOTIFICATIONS`."""
    frame: dict[str, Any] = {"cmd": "list_notifications"}
    if limit is not None:
        frame["limit"] = limit
    if source is not None:
        frame["source"] = source
    if states is not None:
        frame["states"] = states
    return frame

def list_repository_aliases() -> dict[str, Any]:
    """Registered repository aliases. Needs `repository_aliases`."""
    frame: dict[str, Any] = {"cmd": "list_repository_aliases"}
    return frame

def list_runs(*, limit: int) -> dict[str, Any]:
    """Runs this store knows about, newest first."""
    frame: dict[str, Any] = {"cmd": "list_runs"}
    frame["limit"] = limit
    return frame

def list_schedules(*, target: AgentId | None = None) -> dict[str, Any]:
    """The `list_schedules` command."""
    frame: dict[str, Any] = {"cmd": "list_schedules"}
    if target is not None:
        frame["target"] = target
    return frame

def list_tasks(*, query: TaskQuery) -> dict[str, Any]:
    """The `list_tasks` command."""
    frame: dict[str, Any] = {"cmd": "list_tasks"}
    frame["query"] = query
    return frame

def list_workspaces(*, limit: int | None = None) -> dict[str, Any]:
    """Workspaces holding root agents, most recently active first."""
    frame: dict[str, Any] = {"cmd": "list_workspaces"}
    if limit is not None:
        frame["limit"] = limit
    return frame
