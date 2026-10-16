# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from ..protocol import (
    AgentId,
    Any,
)

def workspace_diff(*, agent: AgentId) -> dict[str, Any]:
    """Files, diffstat, commits and a capped patch of an agent's work."""
    frame: dict[str, Any] = {"cmd": "workspace_diff"}
    frame["agent"] = agent
    return frame

def workspace_discard(*, agent: AgentId, force: bool | None = None) -> dict[str, Any]:
    """Throw away an agent's isolated workspace. Refused while an agent using
    it is live unless `force`, which does not stop that agent.
    """
    frame: dict[str, Any] = {"cmd": "workspace_discard"}
    frame["agent"] = agent
    if force is not None:
        frame["force"] = force
    return frame

def workspace_integrate(*, agent: AgentId) -> dict[str, Any]:
    """Land a finished child's isolated workspace in the source it came
    from, through the same conflict check as the `integrate_workspace` tool.
    """
    frame: dict[str, Any] = {"cmd": "workspace_integrate"}
    frame["agent"] = agent
    return frame
