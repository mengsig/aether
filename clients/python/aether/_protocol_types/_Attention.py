# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class Attention(TypedDict, total=False):
    """
    Everything waiting on the operator, read at one sequence number.
    """
    agents: list[_protocol.AgentAttention]
    approvals: list[_protocol.ApprovalRequest]
    last_seq: int
    questions: list[_protocol.PendingQuestion]
