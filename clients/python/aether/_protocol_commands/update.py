# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from ..protocol import (
    AgentId,
    AgentPatch,
    Any,
    DeviceId,
    DeviceRole,
    ScheduleAction,
    ScheduleId,
)

def update_agent(*, id: AgentId, patch: AgentPatch, idempotency_key: str | None = None) -> dict[str, Any]:
    """The `update_agent` command."""
    frame: dict[str, Any] = {"cmd": "update_agent"}
    frame["id"] = id
    if idempotency_key is not None:
        frame["idempotency_key"] = idempotency_key
    frame["patch"] = patch
    return frame

def update_device(*, id: DeviceId, name: str | None = None, role: DeviceRole | None = None) -> dict[str, Any]:
    """Rename a device or change its role. A role change closes its live
    connections. Host only.
    """
    frame: dict[str, Any] = {"cmd": "update_device"}
    frame["id"] = id
    if name is not None:
        frame["name"] = name
    if role is not None:
        frame["role"] = role
    return frame

def update_schedule(*, action: ScheduleAction, id: ScheduleId, idempotency_key: str | None = None) -> dict[str, Any]:
    """The `update_schedule` command."""
    frame: dict[str, Any] = {"cmd": "update_schedule"}
    frame["action"] = action
    frame["id"] = id
    if idempotency_key is not None:
        frame["idempotency_key"] = idempotency_key
    return frame
