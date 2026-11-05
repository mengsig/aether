# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class ConsolidatedCluster(TypedDict, total=False):
    members: list[_protocol.MemoryId]
    outcome: _protocol.ClusterOutcomeKind
    promoted: bool  # optional
    reason: str | None  # optional
    scope: _protocol.MemoryScope
    survivor: _protocol.MemoryId
