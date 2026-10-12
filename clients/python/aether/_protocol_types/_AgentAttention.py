# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class AgentAttention(TypedDict, total=False):
    """
    A failed agent, or one blocked on something only the operator can clear.
    """
    agent: _protocol.AgentId
    agent_name: str | None  # optional
    kind: _protocol.AttentionKind
    reason: str
    since: str
