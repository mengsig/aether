# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class Task(TypedDict, total=False):
    """
    A durable task record.
    """
    artifacts: list[_protocol.ArtifactId]  # optional
    created_at: str
    creator: _protocol.Actor
    delegation_reason: str | None  # optional
    dependencies: list[_protocol.TaskId]  # optional
    finished_at: str | None  # optional
    id: _protocol.TaskId
    objective: str
    owner: _protocol.AgentId | None  # optional
    parent_task: _protocol.TaskId | None  # optional
    priority: _protocol.Priority  # optional
    references: list[_protocol.TaskReference]  # optional
    run: _protocol.RunId
    started_at: str | None  # optional
    status: _protocol.TaskStatus
    updated_at: str
