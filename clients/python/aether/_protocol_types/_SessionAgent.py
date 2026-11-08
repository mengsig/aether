# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class SessionAgent(TypedDict, total=False):
    """
    One agent of a session and the inputs Noesis wrote to it.
    """
    agent: _protocol.AgentId
    internal: list[int]  # optional
    role: _protocol.SessionRole
