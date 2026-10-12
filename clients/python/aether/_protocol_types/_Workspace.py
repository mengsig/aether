# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class Workspace(TypedDict, total=False):
    """
    One workspace the daemon knows: it holds or held a root agent.
    """
    active_at: str
    agents: int
    exists: bool
    git: _protocol.GitSummary | None  # optional
    path: str
    running: int
