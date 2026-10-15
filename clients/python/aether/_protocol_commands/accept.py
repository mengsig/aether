# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from ..protocol import (
    Any,
    MemoryId,
)

def accept_memory(*, id: MemoryId, reason: str | None = None) -> dict[str, Any]:
    """Accept a pending proposal and apply its deferred replacement. Needs `memory_proposals`."""
    frame: dict[str, Any] = {"cmd": "accept_memory"}
    frame["id"] = id
    if reason is not None:
        frame["reason"] = reason
    return frame
