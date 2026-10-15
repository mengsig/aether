# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class ContextLayer(TypedDict, total=False):
    """
    One rendered layer. System layers carry text; the tail carries messages
    and the tool layer carries schemas, so those keep only counts here.
    """
    items: int  # optional
    kind: _protocol.LayerKind
    memories: list[_protocol.MemoryId]  # optional
    placement: _protocol.LayerPlacement  # optional
    text: str | None  # optional
    tokens: int
