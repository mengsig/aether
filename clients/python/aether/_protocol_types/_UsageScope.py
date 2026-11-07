# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class UsageScope(TypedDict, total=False):
    """
    Which invocations `usage_breakdown` sums. Every field given narrows the
    set; none sums the whole fleet.
    """
    agent: _protocol.AgentId | None  # optional
    role_prefix: str | None  # optional
    run: _protocol.RunId | None  # optional
    subtree: _protocol.AgentId | None  # optional
