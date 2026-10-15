# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from ..protocol import (
    Any,
    InvocationId,
    MemoryEvidence,
    MemoryId,
    MemoryVerdict,
    TaskId,
)

def record_memory_feedback(*, evidence: MemoryEvidence, memory: MemoryId, reason: str, task: TaskId, verdict: MemoryVerdict, idempotency_key: str | None = None, invocation: InvocationId | None = None) -> dict[str, Any]:
    """Record host evidence on a memory a task saw. Needs `memory_feedback`."""
    frame: dict[str, Any] = {"cmd": "record_memory_feedback"}
    frame["evidence"] = evidence
    if idempotency_key is not None:
        frame["idempotency_key"] = idempotency_key
    if invocation is not None:
        frame["invocation"] = invocation
    frame["memory"] = memory
    frame["reason"] = reason
    frame["task"] = task
    frame["verdict"] = verdict
    return frame
