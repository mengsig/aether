# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class SpawnSpec(TypedDict, total=False):
    """
    The agent and task each spawn occurrence creates: a root in a new run, or
    its creator's child. Fields as on `create_agent`; no parent or existing run.
    """
    approval: _protocol.ApprovalPolicy | None  # optional
    config: _protocol.AgentConfig | None  # optional
    fallbacks: list[_protocol.ModelRef]  # optional
    model: _protocol.ModelRef | None  # optional
    name: str | None  # optional
    objective: str
    role: str
    scope: _protocol.Scope | None  # optional
