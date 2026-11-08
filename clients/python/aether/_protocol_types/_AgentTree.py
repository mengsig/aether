# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class AgentTree(TypedDict, total=False):
    next_after: _protocol.AgentId | None  # optional
    nodes: dict[str, Any]
    roots: list[_protocol.AgentId]
    stale: _protocol.StatusStale | None  # optional
