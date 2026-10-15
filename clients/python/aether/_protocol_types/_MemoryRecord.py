# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class MemoryRecord(TypedDict, total=False):
    """
    A stored memory.
    """
    accepted_at: str | None  # optional
    confidence: float
    created_at: str
    forgotten_at: str | None  # optional
    id: _protocol.MemoryId
    importance: float
    kind: _protocol.MemoryKind
    last_retrieved_at: str | None  # optional
    pinned: bool  # optional
    proposed_at: str | None  # optional
    provenance: _protocol.MemoryProvenance
    retracted: _protocol.Retraction | None  # optional
    retrieval_count: int  # optional
    scope: _protocol.MemoryScope
    structured: Any  # optional
    superseded_by: _protocol.MemoryId | None  # optional
    supersedes: _protocol.MemoryId | None  # optional
    tags: list[str]  # optional
    text: str
