# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class CreateAgent(TypedDict, total=False):
    """
    Request to create an agent, optionally with an initial task.
    A child requires explicit `config.effort`: low, medium, high, xhigh or max.
    """
    approval: _protocol.ApprovalPolicy | None  # optional
    config: _protocol.AgentConfig | None  # optional
    fallbacks: list[_protocol.ModelRef]  # optional
    idempotency_key: str | None  # optional
    model: _protocol.ModelRef | None  # optional
    name: str | None  # optional
    objective: str | None  # optional
    parent: _protocol.AgentId | None  # optional
    references: list[_protocol.ReferenceRequest]  # optional
    repository_root: str | None  # optional
    role: str
    run: _protocol.RunId | None  # optional
    scope: _protocol.Scope | None  # optional
    start: bool  # optional
