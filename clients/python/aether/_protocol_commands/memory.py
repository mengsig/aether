# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from ..protocol import (
    Any,
    MemoryId,
)

def memory_feedback(*, memory: MemoryId, limit: int | None = None) -> dict[str, Any]:
    """Newest evidence first, including retractions. Needs `memory_feedback`."""
    frame: dict[str, Any] = {"cmd": "memory_feedback"}
    if limit is not None:
        frame["limit"] = limit
    frame["memory"] = memory
    return frame
