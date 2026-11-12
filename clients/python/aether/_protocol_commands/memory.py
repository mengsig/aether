# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from ..protocol import (
    Any,
    InvocationId,
    MemoryId,
    MemoryScope,
    TaskId,
)

def memory_export(*, scope: MemoryScope | None = None) -> dict[str, Any]:
    """Every memory of a scope, or of the store, as a JSON Lines artifact.
    Needs `memory_v2`.
    """
    frame: dict[str, Any] = {"cmd": "memory_export"}
    if scope is not None:
        frame["scope"] = scope
    return frame

def memory_feedback(*, memory: MemoryId, limit: int | None = None) -> dict[str, Any]:
    """Newest evidence first, including retractions. Needs `memory_feedback`."""
    frame: dict[str, Any] = {"cmd": "memory_feedback"}
    if limit is not None:
        frame["limit"] = limit
    frame["memory"] = memory
    return frame

def memory_import(*, artifact: str, dry_run: bool) -> dict[str, Any]:
    """Insert what an export holds and the store lacks; never overwrites.
    Needs `memory_v2`.
    """
    frame: dict[str, Any] = {"cmd": "memory_import"}
    frame["artifact"] = artifact
    frame["dry_run"] = dry_run
    return frame

def memory_migration_status() -> dict[str, Any]:
    """The store's Memory 2.0 migrations, counts and backups. Needs `memory_v2`."""
    frame: dict[str, Any] = {"cmd": "memory_migration_status"}
    return frame

def memory_retrieval_explain(*, invocation: InvocationId | None = None, task: TaskId | None = None) -> dict[str, Any]:
    """What retrieval did for each model call of a task, or for one
    invocation; name exactly one. Needs `memory_v2`.
    """
    frame: dict[str, Any] = {"cmd": "memory_retrieval_explain"}
    if invocation is not None:
        frame["invocation"] = invocation
    if task is not None:
        frame["task"] = task
    return frame
