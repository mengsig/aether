# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class ReplayFrame(TypedDict, total=False):
    """
    One turn of one agent.
    """
    agent: _protocol.AgentId
    approvals: list[_protocol.ReplayApproval]
    context: _protocol.ReplayContext | None  # optional
    duration_ms: int
    ended_at: str | None  # optional
    from_seq: int
    messages: list[_protocol.ReplayMessage]
    notes: list[_protocol.ReplayNote]  # optional
    outcome: _protocol.TurnOutcome
    started_at: str
    to_seq: int
    tools: list[_protocol.ReplayTool]
    turn: int
    usage: _protocol.Usage
