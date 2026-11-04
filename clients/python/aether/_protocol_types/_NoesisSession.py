# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class NoesisSession(TypedDict, total=False):
    active_problem: _protocol.Problem | None  # optional
    agent: _protocol.AgentId
    created_at: str
    goal: str
    id: str
    mode: _protocol.SessionMode
    open_question: _protocol.Question | None  # optional
    phase: _protocol.Phase
    plan: _protocol.PlanTree | None  # optional
    updated_at: str
