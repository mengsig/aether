# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from ..protocol import (
    Any,
    MemoryFeedbackId,
)

def retract_memory_feedback(*, feedback: MemoryFeedbackId, reason: str) -> dict[str, Any]:
    """Withdraw evidence, preserving the record. Needs `memory_feedback`."""
    frame: dict[str, Any] = {"cmd": "retract_memory_feedback"}
    frame["feedback"] = feedback
    frame["reason"] = reason
    return frame
