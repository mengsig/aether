# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class TraceCandidate(TypedDict, total=False):
    memory: _protocol.MemoryId
    outcome: _protocol.TraceOutcome
    pinned: bool
    rank: int | None  # optional
    relevance: float
    score: float | None  # optional
    stale: bool
