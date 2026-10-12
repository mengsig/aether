# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from ..protocol import (
    AgentId,
    Any,
)

def restart(*, agent: AgentId, idempotency_key: str | None = None) -> dict[str, Any]:
    """Recreate a failed or cancelled agent from its last checkpoint, as a
    new agent linked to it.
    """
    frame: dict[str, Any] = {"cmd": "restart"}
    frame["agent"] = agent
    if idempotency_key is not None:
        frame["idempotency_key"] = idempotency_key
    return frame
