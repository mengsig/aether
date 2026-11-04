# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class Problem(TypedDict, total=False):
    """
    A practice problem. Its solution is revealed only by
    `noesis.practice_concluded`.
    """
    id: str
    mode: _protocol.ProblemMode
    posed_at: str
    statement: str
    targets: list[str]
