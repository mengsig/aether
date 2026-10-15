# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from ..protocol import (
    Any,
    MemoryId,
)

def reject_memory(*, id: MemoryId, reason: str) -> dict[str, Any]:
    """Reject a pending proposal with an explanation. Needs `memory_proposals`."""
    frame: dict[str, Any] = {"cmd": "reject_memory"}
    frame["id"] = id
    frame["reason"] = reason
    return frame
