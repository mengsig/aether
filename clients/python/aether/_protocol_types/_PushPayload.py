# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class PushPayload(TypedDict, total=False):
    """
    The decrypted body of one Web Push message.
    """
    agent: _protocol.AgentId | None  # optional
    agent_name: str | None  # optional
    approval: _protocol.ApprovalId | None  # optional
    at: str
    instance_id: str
    kind: _protocol.AttentionKind
    preview: str | None  # optional
    question_seq: int | None  # optional
    seq: int
    task: _protocol.TaskId | None  # optional
    title: str
    url: str
    v: int
