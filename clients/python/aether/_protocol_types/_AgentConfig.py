# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class AgentConfig(TypedDict, total=False):
    """
    Per-agent configuration the runtime enforces.
    """
    approval: _protocol.ApprovalPolicy  # optional
    build_cache: str | None  # optional
    concurrency: int | None  # optional
    cost_budget: float | None  # optional
    deadline: str | None  # optional
    effort: _protocol.Effort | None  # optional
    host_context: _protocol.HostContext  # optional
    instructions: str | None  # optional
    leaf: bool  # optional
    long_lived: bool  # optional
    max_output_tokens: int | None  # optional
    max_turns: int | None  # optional
    priority: _protocol.Priority  # optional
    token_budget: int | None  # optional
    tools: _protocol.ToolExposure  # optional
