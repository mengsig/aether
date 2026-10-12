# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from ..protocol import (
    AgentId,
    Any,
)

def cancel(*, agent: AgentId, idempotency_key: str | None = None, reason: str | None = None) -> dict[str, Any]:
    """The `cancel` command."""
    frame: dict[str, Any] = {"cmd": "cancel"}
    frame["agent"] = agent
    if idempotency_key is not None:
        frame["idempotency_key"] = idempotency_key
    if reason is not None:
        frame["reason"] = reason
    return frame

def cancel_pairing(*, id: str) -> dict[str, Any]:
    """Close a pairing nobody confirmed. Host only."""
    frame: dict[str, Any] = {"cmd": "cancel_pairing"}
    frame["id"] = id
    return frame
