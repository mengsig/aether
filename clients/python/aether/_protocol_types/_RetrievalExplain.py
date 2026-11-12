# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class RetrievalExplain(TypedDict, total=False):
    """
    What retrieval did for each model call of a task, or one invocation.
    """
    truncated: bool  # optional
    turns: list[_protocol.ExplainedTurn]
