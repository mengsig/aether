# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class McpServerHealth(TypedDict, total=False):
    """
    A server's health as of the last contact attempt.
    """
    auth: _protocol.McpAuth | None  # optional
    checked_at: str | None  # optional
    error: str | None  # optional
    name: str
    state: _protocol.McpServerState
    tools: list[_protocol.McpToolInfo]  # optional
    transport: str
