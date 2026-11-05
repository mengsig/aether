# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class ExplainedTurn(TypedDict, total=False):
    agent: _protocol.AgentId
    at: str
    candidates: list[_protocol.TraceCandidate]  # optional
    injected: list[_protocol.MemoryId]  # optional
    invocations: list[_protocol.InvocationId]  # optional
    omitted: _protocol.TraceOmitted  # optional
    policy: _protocol.TracePolicy | None  # optional
    pool: int
    snapshot: _protocol.SnapshotId
    task: _protocol.TaskId | None  # optional
    traced: bool
    turn: int
