# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class ServerFrameDelta(TypedDict, total=False):
    agent: _protocol.AgentId
    delta: _protocol.Delta
    frame: Literal["delta"]
    invocation: _protocol.InvocationId
    invocation_seq: int | None  # optional
    sub: int
