# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class EventKindMemorySuperseded(TypedDict, total=False):
    new: _protocol.MemoryId
    old: _protocol.MemoryId
    reason: str | None  # optional
    type: Literal["memory.superseded"]
