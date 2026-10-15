# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class OperatorMemorySpec(TypedDict, total=False):
    """
    A person supplies content and applicability, never fabricated provenance.
    """
    confidence: float | None  # optional
    importance: float | None  # optional
    kind: _protocol.MemoryKind
    proposed: bool  # optional
    scope: _protocol.OperatorMemoryScope
    supersedes: _protocol.MemoryId | None  # optional
    tags: list[str]  # optional
    text: str
