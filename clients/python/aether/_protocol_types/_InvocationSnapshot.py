# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class InvocationSnapshot(TypedDict, total=False):
    """
    Everything sent to the model on one turn, frozen before the request.

    Remote inference is not deterministic, so this does not promise replay.
    It promises reconstruction: every input the model received, and the
    adapter and capability profile that shaped the wire request, are here.
    """
    adapter: str | None  # optional
    agent_id: _protocol.AgentId
    capabilities: _protocol.CapabilityProfile | None  # optional
    created_at: str
    id: _protocol.SnapshotId
    layers: list[_protocol.ContextLayer]
    memory_ids: list[_protocol.MemoryId]  # optional
    memory_retrieval: _protocol.RetrievalTrace | None  # optional
    messages: list[_protocol.Message]
    model: _protocol.ModelRef
    params: _protocol.InvocationParams
    task_id: _protocol.TaskId | None  # optional
    token_estimate: int
    tools: list[_protocol.ToolSpec]
    turn: int
