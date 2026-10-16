# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class WorkspaceCommit(TypedDict, total=False):
    at: str
    author: str
    id: str
    summary: str
    turn: int | None  # optional
