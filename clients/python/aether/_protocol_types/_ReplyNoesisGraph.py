# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class ReplyNoesisGraph(TypedDict, total=False):
    """
    `truncated` is set when `limit` cut the graph short.
    """
    data: dict[str, Any]
    reply: Literal["noesis_graph"]
