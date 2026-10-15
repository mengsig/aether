# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class SnapshotMemories(TypedDict, total=False):
    """
    The memories one snapshot put in front of the model, as a host asks for
    them through `memories_in_context`.
    """
    agent: _protocol.AgentId
    at: str
    invocations: list[_protocol.InvocationId]  # optional
    memories: list[_protocol.MemoryId]  # optional
    snapshot: _protocol.SnapshotId
    task: _protocol.TaskId | None  # optional
    turn: int
