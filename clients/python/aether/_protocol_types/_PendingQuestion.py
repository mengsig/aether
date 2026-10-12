# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class PendingQuestion(TypedDict, total=False):
    """
    An agent's question nobody has answered or superseded.
    """
    agent: _protocol.AgentId
    agent_name: str | None  # optional
    asked_at: str
    question: str
    question_seq: int
