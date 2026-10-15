# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class MemoryFeedback(TypedDict, total=False):
    actor: _protocol.Actor
    created_at: str
    evidence: _protocol.MemoryEvidence
    id: _protocol.MemoryFeedbackId
    invocation: _protocol.InvocationId | None  # optional
    memory: _protocol.MemoryId
    reason: str
    retracted_at: str | None  # optional
    retraction_reason: str | None  # optional
    task: _protocol.TaskId
    verdict: _protocol.MemoryVerdict
