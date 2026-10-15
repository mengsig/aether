# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class MemoryQuerySpec(TypedDict, total=False):
    id: _protocol.MemoryId | None  # optional
    include_forgotten: bool  # optional
    include_superseded: bool  # optional
    kinds: list[_protocol.MemoryKind]  # optional
    limit: int  # optional
    proposed: bool  # optional
    provenance_agent: _protocol.AgentId | None  # optional
    recorded_by: _protocol.MemoryRecorder | None  # optional
    scope: _protocol.MemoryScope | None  # optional
    tags: list[str]  # optional
    text: str | None  # optional
