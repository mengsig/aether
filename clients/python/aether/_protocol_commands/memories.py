# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from ..protocol import (
    Any,
    InvocationId,
    MemoryQuerySpec,
    TaskId,
)

def memories(*, query: MemoryQuerySpec) -> dict[str, Any]:
    """The `memories` command."""
    frame: dict[str, Any] = {"cmd": "memories"}
    frame["query"] = query
    return frame

def memories_in_context(*, invocation: InvocationId | None = None, task: TaskId | None = None) -> dict[str, Any]:
    """Which stored memories every model call of a task, or one invocation,
    was given. Name exactly one. Needs `memory_context`.
    """
    frame: dict[str, Any] = {"cmd": "memories_in_context"}
    if invocation is not None:
        frame["invocation"] = invocation
    if task is not None:
        frame["task"] = task
    return frame
