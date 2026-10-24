# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class Agent(TypedDict, total=False):
    """
    A durable agent record. This is authoritative state; the model is never
    asked to remember any of it.
    """
    archived: bool  # optional
    checkpoint: _protocol.CheckpointId | None  # optional
    config: _protocol.AgentConfig
    created_at: str
    current_task: _protocol.TaskId | None  # optional
    engine_alive: bool  # optional
    engine_sent: _protocol.EngineSentLayers | None  # optional
    engine_session: str | None  # optional
    engine_usage: _protocol.EngineSessionUsage | None  # optional
    fallbacks: list[_protocol.ModelRef]  # optional
    id: _protocol.AgentId
    inbox_cursor: int  # optional
    last_event_at: str | None  # optional
    model: _protocol.ModelRef
    name: str
    parent: _protocol.AgentId | None  # optional
    paused: bool  # optional
    paused_via: _protocol.AgentId | None  # optional
    restarted_as: _protocol.AgentId | None  # optional
    restarted_from: _protocol.AgentId | None  # optional
    resume_wait: _protocol.WaitCondition | None  # optional
    role: _protocol.Role
    run: _protocol.RunId
    scope: _protocol.Scope
    stats: _protocol.AgentStats  # optional
    status: _protocol.AgentStatus
    status_since: str | None  # optional
    updated_at: str
