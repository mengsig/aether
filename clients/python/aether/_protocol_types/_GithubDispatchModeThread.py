# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class GithubDispatchModeThread(TypedDict, total=False):
    """
    Deliver to (or spawn, the first time or after the previous one went
    terminal) a durable agent keyed by (repository, issue/PR number), so
    the watcher heals itself instead of pointing at a dead agent forever.
    """
    approval: _protocol.ApprovalPolicy | None  # optional
    config: _protocol.AgentConfig | None  # optional
    fallbacks: list[_protocol.ModelRef]  # optional
    mode: Literal["thread"]
    model: _protocol.ModelRef | None  # optional
    name: str | None  # optional
    objective: str
    role: str
    scope: _protocol.Scope | None  # optional
