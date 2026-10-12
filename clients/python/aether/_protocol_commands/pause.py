# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from ..protocol import (
    AgentId,
    Any,
)

def pause(*, agent: AgentId, idempotency_key: str | None = None, subtree: bool | None = None) -> dict[str, Any]:
    """Pause an agent: a durable flag, orthogonal to its status. With
    `subtree` it reaches every live agent beneath it too (§3.1).
    """
    frame: dict[str, Any] = {"cmd": "pause"}
    frame["agent"] = agent
    if idempotency_key is not None:
        frame["idempotency_key"] = idempotency_key
    if subtree is not None:
        frame["subtree"] = subtree
    return frame
